//! Emit next-generation EasyBuild easyconfigs from an existing recipe.
//!
//! Surgical assignment/list rewrites: only `toolchain`, optional application
//! `version`, and named dependency / build-dependency version fields change.
//! All other source bytes stay verbatim.
//!
//! Dependency versions are supplied by the canonical package solver. This
//! module only applies the resulting lock to the source recipe.

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
    /// If the override string starts with a supported comparison operator, it
    /// replaces the entire version field of matching tuples.
    /// Otherwise the operator already present on the source version (if any) is
    /// preserved and only the version token is replaced.
    pub dep_versions: HashMap<String, String>,
    /// Solver-selected dependency toolchains keyed by package name.
    /// Explicit tuples are retargeted, and selections outside the output
    /// hierarchy are made explicit in otherwise implicit tuples.
    pub dep_toolchains: HashMap<String, Toolchain>,
    /// New sha256 for the source tarball, used only when `version` changes.
    /// When `None` and the version changes, the source checksum entry's key
    /// is still renamed to the new versioned tarball name, but the checksum
    /// value is left stale and a warning is added to [`EmitResult::warnings`].
    pub source_checksum: Option<String>,
}

/// Result of a next-generation emit: rewritten text and conventional basename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitResult {
    pub text: String,
    /// EasyBuild filename: `{name}-{version}-{toolchain_name}-{toolchain_version}.eb`
    pub filename: String,
    /// Human-readable warnings about content that was not (or could not be)
    /// safely rewritten, e.g. a stale source checksum or an unreviewed patch
    /// set after a version bump. Callers should surface these to the user.
    pub warnings: Vec<String>,
}

/// Derive the conventional EasyBuild easyconfig basename.
///
/// Shape: `{name}-{version}-{toolchain}-{toolchain_version}{versionsuffix}.eb`.
/// `versionsuffix` already carries its own leading separator (`-ACC`,
/// `-CUDA-12.8.0`), so it is appended verbatim; pass `None` when the recipe
/// declares none. A recipe whose `versionsuffix` still contains an unresolved
/// `%(...)s` template cannot be named from text alone, so callers pass `None`
/// and warn instead of emitting a filename with a literal template in it.
pub fn easyconfig_filename(
    name: &str,
    version: &str,
    toolchain: &Toolchain,
    versionsuffix: Option<&str>,
) -> String {
    format!(
        "{}-{}-{}-{}{}.eb",
        name,
        version,
        toolchain.name,
        toolchain.version,
        versionsuffix.unwrap_or("")
    )
}

/// Whether a raw easyconfig value still holds an EasyBuild `%(key)s` template.
///
/// Templated values cannot be resolved by textual rewriting: `-CUDA-%(cudaver)s`
/// only becomes `-CUDA-12.8.0` once EasyBuild has the CUDA dependency in hand.
fn has_unresolved_template(value: &str) -> bool {
    value.contains("%(")
}

/// Emit next-generation easyconfig text and conventional filename from source text.
pub fn emit_next_generation(source: &str, params: &EmitParams) -> Result<EmitResult, EmitError> {
    let name = assign_string_raw(source, "name").ok_or(EmitError::MissingName)?;
    let source_version = assign_string_raw(source, "version");
    let app_version = match &params.version {
        Some(v) => v.clone(),
        None => source_version.clone().ok_or(EmitError::MissingVersion)?,
    };

    // Ensure source has a toolchain assignment we can rewrite.
    if !source.lines().any(|l| {
        let t = l.trim();
        t.starts_with("toolchain") && t.contains('=')
    }) {
        return Err(EmitError::MissingToolchain);
    }

    // A version bump changes the source tarball name; a toolchain-only bump
    // (params.version == None, or explicitly set back to the source version)
    // does not, so checksums/patches stay untouched in that case.
    let version_changed =
        params.version.is_some() && source_version.as_deref() != Some(app_version.as_str());

    let mut text = source.to_string();
    text = rewrite_toolchain(&text, &params.toolchain)?;
    if params.version.is_some() {
        text = rewrite_string_assign(&text, "version", &app_version)?;
    }
    if !params.dep_versions.is_empty() || !params.dep_toolchains.is_empty() {
        text = rewrite_dep_list_selections(
            &text,
            "dependencies",
            &params.dep_versions,
            &params.dep_toolchains,
            &params.toolchain,
        )?;
        text = rewrite_dep_list_selections(
            &text,
            "builddependencies",
            &params.dep_versions,
            &params.dep_toolchains,
            &params.toolchain,
        )?;
    }

    let mut warnings = Vec::new();
    if version_changed {
        if let Some(old_v) = source_version.as_deref() {
            let rewrite = rewrite_source_checksum(
                &text,
                old_v,
                &app_version,
                params.source_checksum.as_deref(),
            )?;
            text = rewrite.text;
            if let Some(note) = rewrite.note {
                warnings.push(note);
            }
            if rewrite.stale {
                warnings.push(format!(
                    "source checksum is stale after version bump {old_v} -> {app_version}: \
                     the tarball key was renamed but the checksum value was left unchanged; \
                     set --source-checksum <SHA256> or run `eb --inject-checksums` before building"
                ));
            }
        }
        if list_is_nonempty(&text, "patches") {
            warnings.push(format!(
                "patches were not modified for version bump to {app_version}: \
                 review patch applicability -- a version bump commonly needs a different patch set"
            ));
        }
    }

    // The rewritten text keeps whatever `versionsuffix` the source declared, so
    // the filename has to carry it too or the emitted basename disagrees with the
    // recipe it names (and with the name a build list refers to).
    let versionsuffix = assign_string_raw(&text, "versionsuffix");
    let filename_suffix = match versionsuffix.as_deref() {
        Some(suffix) if has_unresolved_template(suffix) => {
            warnings.push(format!(
                "versionsuffix {suffix:?} contains an unresolved template, so it is \
                 omitted from the emitted filename: rename the file to match the \
                 resolved suffix before building"
            ));
            None
        }
        other => other,
    };
    let filename = easyconfig_filename(&name, &app_version, &params.toolchain, filename_suffix);
    Ok(EmitResult {
        text,
        filename,
        warnings,
    })
}

/// Read a source file and emit the next-generation recipe.
pub fn emit_next_generation_from_path(
    path: &std::path::Path,
    params: &EmitParams,
) -> Result<EmitResult, EmitError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| EmitError::Rewrite(format!("read {}: {}", path.display(), e)))?;
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

    let bytes = src.as_bytes();
    // Find byte offset of start of each line (used by both the bare-SYSTEM and
    // dict-rewrite paths below).
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

    // Handle the bare `toolchain = SYSTEM` form (no dict braces to scan). This
    // is how SYSTEM-toolchain recipes (e.g. nvidia-compilers, CMake) declare
    // their toolchain.
    {
        let rhs = lines[start_line]
            .split_once('=')
            .map(|(_, r)| r)
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if rhs == "SYSTEM" {
            // Target is also SYSTEM (a version-only bump): leave it untouched.
            if tc.name.eq_ignore_ascii_case("system") {
                return Ok(src.to_string());
            }
            // Promote SYSTEM to a real toolchain dict in place, preserving the
            // rest of the line (indentation, trailing comment).
            let in_line = lines[start_line]
                .find("SYSTEM")
                .expect("rhs == SYSTEM implies the token is present");
            let abs = line_offsets[start_line] + in_line;
            let replacement = format!("{{'name': '{}', 'version': '{}'}}", tc.name, tc.version);
            let mut out = String::with_capacity(src.len() + replacement.len());
            out.push_str(&src[..abs]);
            out.push_str(&replacement);
            out.push_str(&src[abs + "SYSTEM".len()..]);
            return Ok(out);
        }
    }

    // Rebuild with name/version values substituted inside the dict, preserving
    // surrounding whitespace and quote characters on each key/value pair.
    let mut capturing = false;
    let mut depth = 0i32;
    let mut span_start = None;
    let mut span_end = None;

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

/// Locate a top-level `key = [ ... ]` assignment and return the byte offsets
/// `(list_open_end, list_close_start)`: just after the opening `[` and at the
/// matching closing `]`. Returns `None` when no such assignment exists.
/// Span of the whole `key = [ ... ]` assignment: from the start of its line
/// to just past the closing bracket. Callers splicing entire list blocks
/// (patch adoption) need the assignment, not only the body.
pub(crate) fn find_list_assignment_span(
    src: &str,
    key: &str,
) -> Result<Option<(usize, usize)>, EmitError> {
    let re_hdr = regex::Regex::new(&format!(r"(?m)^(\s*{}\s*=\s*\[)", regex::escape(key)))
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let Some(m) = re_hdr.find(src) else {
        return Ok(None);
    };
    let Some((_, body_end)) = find_list_span(src, key)? else {
        return Ok(None);
    };
    Ok(Some((m.start(), body_end + 1)))
}

fn find_list_span(src: &str, key: &str) -> Result<Option<(usize, usize)>, EmitError> {
    let re_hdr = regex::Regex::new(&format!(r"(?m)^(\s*{}\s*=\s*\[)", regex::escape(key)))
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let Some(m) = re_hdr.find(src) else {
        return Ok(None);
    };
    let list_open_end = m.end(); // index just after '['
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut i = list_open_end;
    // Brackets only count outside strings and comments: a '[' in a quoted
    // filename or a ']' in a trailing comment must not move the span.
    let mut in_string: Option<u8> = None;
    let mut in_comment = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_comment {
            if c == b'\n' {
                in_comment = false;
            }
        } else if let Some(quote) = in_string {
            if c == b'\\' {
                i += 1; // skip the escaped byte
            } else if c == quote {
                in_string = None;
            }
        } else {
            match c {
                b'\'' | b'"' => in_string = Some(c),
                b'#' => in_comment = true,
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    if depth != 0 {
        return Err(EmitError::Rewrite(format!("unclosed {key} list")));
    }
    Ok(Some((list_open_end, i)))
}

/// Whether `key = [ ... ]` exists in `src` and has at least one non-whitespace
/// element (used to decide whether a "review the patch set" warning applies).
fn list_is_nonempty(src: &str, key: &str) -> bool {
    matches!(find_list_span(src, key), Ok(Some((s, e))) if !src[s..e].trim().is_empty())
}

/// Apply solver-selected versions and toolchains inside `key = [ ... ]`.
fn rewrite_dep_list_selections(
    src: &str,
    key: &str,
    version_overrides: &HashMap<String, String>,
    toolchain_overrides: &HashMap<String, Toolchain>,
    target_toolchain: &Toolchain,
) -> Result<String, EmitError> {
    let Some((list_open_end, list_close_start)) = find_list_span(src, key)? else {
        // No such list — nothing to rewrite (not an error).
        return Ok(src.to_string());
    };
    let body = &src[list_open_end..list_close_start];
    let new_body = rewrite_dep_tuples_in_body(
        body,
        version_overrides,
        toolchain_overrides,
        target_toolchain,
    )?;
    let mut out = String::with_capacity(src.len() + 32);
    out.push_str(&src[..list_open_end]);
    out.push_str(&new_body);
    out.push_str(&src[list_close_start..]);
    Ok(out)
}

/// Rewrite the SOURCE tarball entry (the first element) inside a
/// `checksums = [ ... ]` list after a version bump. Handles both the dict
/// form (`{'name-version.tar.bz2': 'sha256'}`) and a bare-string checksum
/// form. Only the first (source) element is touched; patch checksum entries
/// that follow are left untouched.
///
/// The key (tarball filename), when present, has its first occurrence of
/// `old_version` replaced with `new_version`. The checksum value is replaced
/// with `new_checksum` when given; otherwise it is left as-is and the second
/// return value is `true` (the value is now stale).
/// Advance past whitespace and whole-line `#` comments to the first real entry.
///
/// An annotated checksums list names each file above its hash, so the first
/// thing inside the brackets is often a comment rather than an entry.
fn skip_to_first_entry(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    loop {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        return i;
    }
}

/// Checksum type names EasyBuild accepts as the first element of a
/// `('<type>', '<value>')` entry. The list decides whether a two-element tuple
/// is a typed checksum or a pair of alternative values, which is the only way to
/// tell those two shapes apart.
const CHECKSUM_TYPES: [&str; 7] = [
    "adler32", "crc32", "md5", "sha1", "sha256", "sha512", "size",
];

fn is_checksum_type(token: &str) -> bool {
    CHECKSUM_TYPES.contains(&token)
}

/// Quoted spans in source order, as (start, end, quote) with `end` exclusive of
/// the closing quote's successor. Backslash escapes inside a span are honoured.
fn quoted_tokens(s: &str) -> Vec<(usize, usize, char)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            q @ (b'\'' | b'"') => {
                let start = i;
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    if escaped {
                        escaped = false;
                    } else if bytes[i] == b'\\' {
                        escaped = true;
                    } else if bytes[i] == q {
                        break;
                    }
                    i += 1;
                }
                let end = (i + 1).min(bytes.len());
                out.push((start, end, q as char));
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

/// Span of one bracketed group starting at `start`, respecting quotes and
/// comments so a delimiter inside a string cannot close it early.
fn balanced_span(bytes: &[u8], start: usize, open: u8, close: u8) -> usize {
    let mut depth = 0i32;
    let mut i = start;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut comment = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if comment {
            if byte == b'\n' {
                comment = false;
            }
        } else if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
        } else if byte == b'#' {
            comment = true;
        } else if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    bytes.len()
}

/// A short, single-line rendering of an entry, for error messages that have to
/// name the form they rejected.
fn entry_excerpt(entry: &str) -> String {
    let line = entry.lines().next().unwrap_or("").trim();
    if line.chars().count() > 40 {
        let cut: String = line.chars().take(40).collect();
        format!("{cut}...")
    } else {
        line.to_string()
    }
}

/// Span of the checksum entry beginning at `start`, whichever documented shape
/// it takes.
fn entry_span(bytes: &[u8], start: usize) -> Result<usize, EmitError> {
    match bytes[start] {
        b'{' => Ok(balanced_span(bytes, start, b'{', b'}')),
        b'(' => Ok(balanced_span(bytes, start, b'(', b')')),
        b'[' => Ok(balanced_span(bytes, start, b'[', b']')),
        b'\'' | b'"' => {
            let s = String::from_utf8_lossy(&bytes[start..]).to_string();
            match quoted_tokens(&s).first() {
                Some(&(_, end, _)) => Ok(start + end),
                None => Ok(bytes.len()),
            }
        }
        _ => {
            let rest = String::from_utf8_lossy(&bytes[start..]).to_string();
            Err(EmitError::Rewrite(format!(
                "unrecognized source checksum entry form: expected a quoted checksum, \
                 a ('<type>', '<value>') tuple, a list of checksums, or a \
                 {{'<file>': <checksum>}} dict, found `{}`",
                entry_excerpt(&rest)
            )))
        }
    }
}

/// Outcome of rewriting one checksum value expression.
struct ValueRewrite {
    text: String,
    /// The value could not be updated because no new checksum was supplied.
    stale: bool,
    /// Set when several checksums were replaced by the one new value, which a
    /// reviewer has to see: alternatives and AND-lists carry information that a
    /// single post-bump hash cannot.
    note: Option<String>,
}

/// Rewrite one checksum *value*: a bare quoted hash, a typed
/// `('<type>', '<value>')` tuple, a tuple of alternative values, or a list of
/// values that all have to match. Used both for a top-level entry and for the
/// value inside a `{'<file>': ...}` dict, which may itself be any of these.
fn rewrite_checksum_value(
    expr: &str,
    new_checksum: Option<&str>,
) -> Result<ValueRewrite, EmitError> {
    let trimmed = expr.trim();
    let Some(first) = trimmed.chars().next() else {
        return Err(EmitError::Rewrite(
            "empty source checksum value".to_string(),
        ));
    };
    let lead = expr.len() - expr.trim_start().len();
    let tail = expr.len() - expr.trim_end().len();
    let rebuild = |body: String| format!("{}{}{}", &expr[..lead], body, &expr[expr.len() - tail..]);

    // A single quoted hash: replace it, or report it stale.
    if first == '\'' || first == '"' {
        return Ok(match new_checksum {
            Some(sha) => ValueRewrite {
                text: rebuild(format!("{first}{sha}{first}")),
                stale: false,
                note: None,
            },
            None => ValueRewrite {
                text: expr.to_string(),
                stale: true,
                note: None,
            },
        });
    }

    if first == '(' || first == '[' {
        let tokens = quoted_tokens(trimmed);
        let typed_pair = first == '('
            && tokens.len() == 2
            && is_checksum_type(&trimmed[tokens[0].0 + 1..tokens[0].1 - 1]);
        if typed_pair {
            // Keep the declared type, replace only the value.
            let (vstart, vend, vquote) = tokens[1];
            return Ok(match new_checksum {
                Some(sha) => {
                    let mut body = String::with_capacity(trimmed.len() + 16);
                    body.push_str(&trimmed[..vstart]);
                    body.push(vquote);
                    body.push_str(sha);
                    body.push(vquote);
                    body.push_str(&trimmed[vend..]);
                    ValueRewrite {
                        text: rebuild(body),
                        stale: false,
                        note: None,
                    }
                }
                None => ValueRewrite {
                    text: expr.to_string(),
                    stale: true,
                    note: None,
                },
            });
        }

        // Alternatives (tuple) or an all-must-match list. After a version bump
        // every one of them is wrong, so collapse to the single new value and
        // say so; without a new value, leave the shape alone and report stale.
        return Ok(match new_checksum {
            Some(sha) => {
                let kind = if first == '(' {
                    "alternative checksums"
                } else {
                    "checksums that all had to match"
                };
                ValueRewrite {
                    text: rebuild(format!("'{sha}'")),
                    stale: false,
                    note: Some(format!(
                        "source checksum entry held {} {kind} and was replaced by the single \
                         value given: review whether the new version needs the same shape",
                        tokens.len()
                    )),
                }
            }
            None => ValueRewrite {
                text: expr.to_string(),
                stale: true,
                note: None,
            },
        });
    }

    Err(EmitError::Rewrite(format!(
        "unrecognized source checksum value form: expected a quoted checksum, \
         a ('<type>', '<value>') tuple, or a list of checksums, found `{}`",
        entry_excerpt(trimmed)
    )))
}

/// Outcome of rewriting the source entry of a `checksums` list.
struct ChecksumRewrite {
    text: String,
    stale: bool,
    note: Option<String>,
}

fn rewrite_source_checksum(
    src: &str,
    old_version: &str,
    new_version: &str,
    new_checksum: Option<&str>,
) -> Result<ChecksumRewrite, EmitError> {
    let Some((list_open_end, list_close_start)) = find_list_span(src, "checksums")? else {
        // No checksums list — nothing to rewrite (not an error).
        return Ok(ChecksumRewrite {
            text: src.to_string(),
            stale: false,
            note: None,
        });
    };
    let body = &src[list_open_end..list_close_start];
    let body_bytes = body.as_bytes();

    let start = skip_to_first_entry(body_bytes, 0);
    if start >= body_bytes.len() {
        // Empty checksums list — nothing to rewrite.
        return Ok(ChecksumRewrite {
            text: src.to_string(),
            stale: false,
            note: None,
        });
    }

    let elem_end = entry_span(body_bytes, start)?;
    let elem = &body[start..elem_end];

    let (new_elem, stale, note) = if elem.starts_with('{') {
        // Dict form: the first quoted string is the artifact filename, and
        // everything after the colon that follows it is the checksum value,
        // which may itself be a bare hash, a typed tuple, or a list.
        let tokens = quoted_tokens(elem);
        let Some(&(key_start, key_end, key_quote)) = tokens.first() else {
            return Err(EmitError::Rewrite(format!(
                "malformed source checksum dict entry: no quoted filename key in `{}`",
                entry_excerpt(elem)
            )));
        };
        let Some(colon_rel) = elem[key_end..].find(':') else {
            return Err(EmitError::Rewrite(format!(
                "malformed source checksum dict entry: no `:` after the filename key in `{}`",
                entry_excerpt(elem)
            )));
        };
        let value_start = key_end + colon_rel + 1;
        let Some(brace_rel) = elem.rfind('}') else {
            return Err(EmitError::Rewrite(format!(
                "malformed source checksum dict entry: unterminated dict in `{}`",
                entry_excerpt(elem)
            )));
        };
        if brace_rel < value_start {
            return Err(EmitError::Rewrite(format!(
                "malformed source checksum dict entry: no value between `:` and `}}` in `{}`",
                entry_excerpt(elem)
            )));
        }

        let key_str = &elem[key_start + 1..key_end - 1];
        let new_key = if key_str.contains(old_version) {
            key_str.replacen(old_version, new_version, 1)
        } else {
            key_str.to_string()
        };
        let value = rewrite_checksum_value(&elem[value_start..brace_rel], new_checksum)?;

        let mut out = String::with_capacity(elem.len() + 16);
        out.push_str(&elem[..key_start]);
        out.push(key_quote);
        out.push_str(&new_key);
        out.push(key_quote);
        out.push_str(&elem[key_end..value_start]);
        out.push_str(&value.text);
        out.push_str(&elem[brace_rel..]);
        (out, value.stale, value.note)
    } else {
        // Bare hash, typed tuple, alternatives tuple, or AND-list: no filename
        // key to rename.
        let value = rewrite_checksum_value(elem, new_checksum)?;
        (value.text, value.stale, value.note)
    };

    let mut out = String::with_capacity(src.len() + 16);
    out.push_str(&src[..list_open_end + start]);
    out.push_str(&new_elem);
    out.push_str(&src[list_open_end + elem_end..]);
    Ok(ChecksumRewrite {
        text: out,
        stale,
        note,
    })
}

#[derive(Debug)]
struct QuotedToken {
    start: usize,
    end: usize,
    depth: usize,
    quote: char,
}

fn rewrite_dep_tuples_in_body(
    body: &str,
    version_overrides: &HashMap<String, String>,
    toolchain_overrides: &HashMap<String, Toolchain>,
    target_toolchain: &Toolchain,
) -> Result<String, EmitError> {
    let mut rewritten = body.to_string();
    for (start, end) in dependency_tuple_spans(body)?.into_iter().rev() {
        let tuple = &body[start..end];
        let replacement = rewrite_dependency_tuple(
            tuple,
            version_overrides,
            toolchain_overrides,
            target_toolchain,
        )?;
        if replacement != tuple {
            rewritten.replace_range(start..end, &replacement);
        }
    }
    Ok(rewritten)
}

fn dependency_tuple_spans(body: &str) -> Result<Vec<(usize, usize)>, EmitError> {
    let bytes = body.as_bytes();
    let mut spans = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            continue;
        }
        match byte {
            b'#' => comment = true,
            b'\'' | b'"' => quote = Some(byte),
            b'(' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            b')' => {
                if depth == 0 {
                    return Err(EmitError::Rewrite(
                        "unbalanced dependency tuple closing parenthesis".into(),
                    ));
                }
                depth -= 1;
                if depth == 0 {
                    spans.push((start.take().expect("tuple start"), index + 1));
                }
            }
            _ => {}
        }
    }
    if depth != 0 || quote.is_some() {
        return Err(EmitError::Rewrite(
            "unclosed dependency tuple or string".into(),
        ));
    }
    Ok(spans)
}

fn rewrite_dependency_tuple(
    tuple: &str,
    version_overrides: &HashMap<String, String>,
    toolchain_overrides: &HashMap<String, Toolchain>,
    target_toolchain: &Toolchain,
) -> Result<String, EmitError> {
    let (tokens, outer_commas) = dependency_tuple_tokens(tuple)?;
    let top = tokens
        .iter()
        .filter(|token| token.depth == 1)
        .collect::<Vec<_>>();
    if top.len() < 2 {
        return Ok(tuple.to_string());
    }
    let name = &tuple[top[0].start..top[0].end];
    let version_override = version_overrides.get(name);
    let toolchain_override = toolchain_overrides.get(name);
    if version_override.is_none() && toolchain_override.is_none() {
        return Ok(tuple.to_string());
    }

    let mut edits = Vec::new();
    if let Some(version) = version_override {
        edits.push((
            top[1].start,
            top[1].end,
            apply_version_override(&tuple[top[1].start..top[1].end], version),
        ));
    }
    if let Some(toolchain) = toolchain_override {
        let nested = tokens
            .iter()
            .filter(|token| token.depth == 2)
            .collect::<Vec<_>>();
        if nested.len() >= 2 {
            edits.push((nested[0].start, nested[0].end, toolchain.name.clone()));
            edits.push((nested[1].start, nested[1].end, toolchain.version.clone()));
        } else if dependency_toolchain_must_be_explicit(toolchain, target_toolchain) {
            if outer_commas >= 3 {
                return Err(EmitError::Rewrite(format!(
                    "dependency {name} has an unsupported explicit toolchain expression"
                )));
            }
            let quote = top[0].quote;
            let suffix = if outer_commas == 1 {
                format!(
                    ", {quote}{quote}, ({quote}{}{quote}, {quote}{}{quote})",
                    toolchain.name, toolchain.version
                )
            } else {
                format!(
                    ", ({quote}{}{quote}, {quote}{}{quote})",
                    toolchain.name, toolchain.version
                )
            };
            edits.push((tuple.len() - 1, tuple.len() - 1, suffix));
        }
    }

    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut rewritten = tuple.to_string();
    for (start, end, replacement) in edits {
        rewritten.replace_range(start..end, &replacement);
    }
    Ok(rewritten)
}

fn dependency_tuple_tokens(tuple: &str) -> Result<(Vec<QuotedToken>, usize), EmitError> {
    let bytes = tuple.as_bytes();
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut token_start = 0usize;
    let mut escaped = false;
    let mut comment = false;
    let mut outer_commas = 0usize;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                tokens.push(QuotedToken {
                    start: token_start,
                    end: index,
                    depth,
                    quote: active_quote as char,
                });
                quote = None;
            }
            continue;
        }
        match byte {
            b'#' => comment = true,
            b'\'' | b'"' => {
                quote = Some(byte);
                token_start = index + 1;
            }
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 1 => outer_commas += 1,
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(EmitError::Rewrite(
            "unclosed string in dependency tuple".into(),
        ));
    }
    Ok((tokens, outer_commas))
}

fn dependency_toolchain_must_be_explicit(selected: &Toolchain, target: &Toolchain) -> bool {
    if crate::hierarchy::is_system_toolchain(target) {
        return !crate::hierarchy::is_system_toolchain(selected);
    }
    if crate::hierarchy::is_system_toolchain(selected) {
        return true;
    }
    crate::hierarchy::hierarchy_for(target, None)
        .map(|hierarchy| !hierarchy.contains(selected))
        .unwrap_or_else(|_| !crate::hierarchy::toolchains_match(selected, target))
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

    fn nvofbf(ver: &str) -> Toolchain {
        Toolchain {
            name: "nvofbf".into(),
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
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(MINIMAL, &params).expect("emit");
        assert_eq!(r.filename, "GROMACS-2024.1-foss-2025b.eb");
        assert!(r
            .text
            .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
        assert!(r.text.contains("version = '2024.1'"));
        assert!(r
            .text
            .contains("toolchainopts = {'openmp': True, 'usempi': True}"));
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
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(MINIMAL, &params).expect("emit");
        assert_eq!(r.filename, "GROMACS-2025.0-foss-2025b.eb");
        assert!(r.text.contains("version = '2025.0'"));
        assert!(r
            .text
            .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
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
            dep_toolchains: HashMap::new(),
            source_checksum: None,
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
            dep_toolchains: HashMap::new(),
            source_checksum: None,
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
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(WITH_BUILDDEPS, &params).expect("emit");
        assert!(r.text.contains("('OpenMPI', '==5.0.3')"));
    }

    #[test]
    fn filename_helper_matches_fixture_shape() {
        assert_eq!(
            easyconfig_filename("GROMACS", "2025.0", &foss("2025b"), None),
            "GROMACS-2025.0-foss-2025b.eb"
        );
    }

    #[test]
    fn filename_helper_appends_versionsuffix() {
        assert_eq!(
            easyconfig_filename("VASP6", "6.6.1", &nvofbf("2025.10"), Some("-ACC")),
            "VASP6-6.6.1-nvofbf-2025.10-ACC.eb"
        );
    }

    /// A recipe carrying `versionsuffix` must be named for it. Emitting
    /// `VASP6-6.6.1-nvofbf-2025.10.eb` for a recipe whose body says
    /// `versionsuffix = '-ACC'` gives a basename that does not match the recipe
    /// it names, and does not match what a build list asks for.
    #[test]
    fn emitted_filename_carries_source_versionsuffix() {
        let src = "name = 'VASP6'\nversion = '6.5.1'\nversionsuffix = '-ACC'\n\
                   toolchain = {'name': 'nvofbf', 'version': '2025.10'}\n";
        let params = EmitParams {
            toolchain: nvofbf("2025.10"),
            version: Some("6.6.1".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(src, &params).expect("emit");
        assert_eq!(r.filename, "VASP6-6.6.1-nvofbf-2025.10-ACC.eb");
        // The body still declares it, so body and basename agree.
        assert!(r.text.contains("versionsuffix = '-ACC'"));
    }

    #[test]
    fn emitted_filename_omits_templated_versionsuffix_and_warns() {
        let src = "name = 'NVHPC'\nversion = '25.3'\nversionsuffix = '-CUDA-%(cudaver)s'\n\
                   toolchain = {'name': 'foss', 'version': '2025b'}\n";
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: None,
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(src, &params).expect("emit");
        // Never emit a literal %(cudaver)s into a filename.
        assert_eq!(r.filename, "NVHPC-25.3-foss-2025b.eb");
        assert!(
            r.warnings.iter().any(|w| w.contains("unresolved template")),
            "expected a warning about the templated versionsuffix, got {:?}",
            r.warnings
        );
    }

    #[test]
    fn emitted_filename_has_no_suffix_when_source_declares_none() {
        let src = "name = 'Pkg'\nversion = '1.0'\n\
                   toolchain = {'name': 'foss', 'version': '2025a'}\n";
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: None,
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(src, &params).expect("emit");
        assert_eq!(r.filename, "Pkg-1.0-foss-2025b.eb");
        assert!(r.warnings.iter().all(|w| !w.contains("versionsuffix")));
    }

    #[test]
    fn double_quoted_style_preserved() {
        let src = "name = \"Pkg\"\nversion = \"1.0\"\ntoolchain = {\"name\": \"foss\", \"version\": \"2025a\"}\n";
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: Some("2.0".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(src, &params).expect("emit");
        assert_eq!(r.filename, "Pkg-2.0-foss-2025b.eb");
        assert!(r.text.contains("version = \"2.0\""));
        assert!(r
            .text
            .contains("toolchain = {\"name\": \"foss\", \"version\": \"2025b\"}"));
    }

    const WITH_CHECKSUMS: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
patches = [
    'OpenMPI-5.0.3_fix_hle_make_errors.patch',
]
checksums = [
    {'openmpi-5.0.3.tar.bz2': '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b'},
    {'OpenMPI-5.0.3_fix_hle_make_errors.patch': '881c907a9f5901d5d6af41cd33dffdcecba4a67a9e5123e602542aea57a80895'},
]
dependencies = [
    ('hwloc', '2.10.0'),
]
";

    /// A checksums list that names each file in a comment above its hash, the
    /// shape upstream uses for annotated multi-entry lists.
    const WITH_COMMENTED_CHECKSUMS: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
patches = [
    'OpenMPI-5.0.3_fix_hle_make_errors.patch',
]
checksums = [
    # openmpi-5.0.3.tar.bz2
    {'openmpi-5.0.3.tar.bz2': '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b'},
    # OpenMPI-5.0.3_fix_hle_make_errors.patch
    {'OpenMPI-5.0.3_fix_hle_make_errors.patch': '881c907a9f5901d5d6af41cd33dffdcecba4a67a9e5123e602542aea57a80895'},
]
dependencies = [
    ('hwloc', '2.10.0'),
]
";

    /// The same list with a bare-string first entry under its comment.
    const WITH_COMMENTED_BARE_CHECKSUM: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    # openmpi-5.0.3.tar.bz2

    '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b',
]
";

    /// Typed entry: `('<type>', '<value>')`. EasyBuild reads the first element
    /// as the checksum type, so only the second may be rewritten.
    const WITH_TYPED_TUPLE_CHECKSUM: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    ('sha256', '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b'),
]
";

    /// Alternative checksums: a tuple whose elements are values, not a type and
    /// a value. Any one of them matching is enough for EasyBuild.
    const WITH_ALTERNATIVE_CHECKSUMS: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    ('990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b',
     '2f1c0b6b1e6d0e6b6c1f4e5a9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e1d0c'),
]
";

    /// A list nested inside the entry: every checksum in it has to match.
    const WITH_AND_LIST_CHECKSUM: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    ['990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b',
     ('md5', 'd0ca4e0f6ac0b9e0f0f0a0b0c0d0e0f0')],
]
";

    /// The dict form carrying a typed tuple as its value, which is where a
    /// position-based reader silently overwrites the type instead of the hash.
    const WITH_DICT_TYPED_TUPLE_CHECKSUM: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    {'openmpi-5.0.3.tar.bz2': ('sha256', '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b')},
]
";

    /// The recipe behind the scoreboard's one ERROR row, reduced to the fields
    /// that drive the rewrite. Bytes taken from
    /// `easybuild/easyconfigs/k/Kokkos/Kokkos-4.7.01-GCC-14.3.0.eb` at
    /// easybuild-easyconfigs 7cab6860, the merge-base of PR #25938: annotated
    /// bare-string checksums, source plus one patch.
    const KOKKOS_ANNOTATED_BARE_CHECKSUMS: &str = "\
name = 'Kokkos'
version = '4.7.01'
toolchain = {'name': 'GCC', 'version': '14.3.0'}
homepage = 'https://github.com/kokkos/kokkos'
sources = [{'download_filename': '%(version)s.tar.gz', 'filename': SOURCE_TAR_GZ}]
patches = ['Kokkos-4.7.01_fix-missing-check-include-cxx.patch']
checksums = [
    # Kokkos-4.7.01.tar.gz
    'cebf6daeb99c95e3d4116ea0d97c94b6a521c0cff1f5e613f127f6beea960ac7',
    # Kokkos-4.7.01_fix-missing-check-include-cxx.patch
    'ef8bb57088970fb765acd00bee6d9a07b52383b5e890feb7679bdff3cb1ad0db',
]
";

    /// A form no EasyBuild version accepts, to pin the error message.
    const WITH_UNKNOWN_CHECKSUM_FORM: &str = "\
name = 'OpenMPI'
version = '5.0.3'
toolchain = {'name': 'NVHPC', 'version': '24.9-CUDA-12.6.0'}
homepage = 'https://www.open-mpi.org/'
sources = [SOURCELOWER_TAR_BZ2]
checksums = [
    CHECKSUM_FROM_ELSEWHERE,
]
";

    fn nvhpc(ver: &str) -> Toolchain {
        Toolchain {
            name: "NVHPC".into(),
            version: ver.into(),
        }
    }

    #[test]
    fn list_span_ignores_brackets_in_comments_and_strings() {
        let src = "patches = [\n    'weird[1].patch',  # see [upstream]\n    \"other]name.patch\",\n]\nmoduleclass = 'lib'\n";
        let (s, e) = find_list_span(src, "patches").unwrap().unwrap();
        let body = &src[s..e];
        assert!(body.contains("weird[1].patch"), "{body}");
        assert!(body.contains("other]name.patch"), "{body}");
        // The span closes at the real bracket, before moduleclass.
        assert!(!body.contains("moduleclass"), "{body}");
    }

    #[test]
    fn list_span_still_errors_on_a_genuinely_unclosed_list() {
        let src = "patches = [\n    'a.patch',\n";
        assert!(find_list_span(src, "patches").is_err());
    }

    #[test]
    fn version_bump_with_source_checksum_rewrites_source_entry() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: Some(
                "119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd".into(),
            ),
        };
        let r = emit_next_generation(WITH_CHECKSUMS, &params).expect("emit");
        assert_eq!(r.filename, "OpenMPI-5.0.7-NVHPC-25.11-CUDA-12.8.0.eb");
        // Source tarball key renamed to the new version and checksum replaced.
        assert!(r.text.contains(
            "{'openmpi-5.0.7.tar.bz2': '119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd'}"
        ));
        // Patch checksum entry left untouched.
        assert!(r.text.contains(
            "{'OpenMPI-5.0.3_fix_hle_make_errors.patch': '881c907a9f5901d5d6af41cd33dffdcecba4a67a9e5123e602542aea57a80895'}"
        ));
        // No stale-checksum warning since a value was supplied, but the
        // patch set still needs human review after a version bump.
        assert_eq!(r.warnings.len(), 1, "warnings: {:?}", r.warnings);
        assert!(r.warnings[0].contains("patches"), "{:?}", r.warnings);
    }

    #[test]
    fn a_comment_above_the_first_checksum_does_not_stop_the_rewrite() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: Some(
                "119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd".into(),
            ),
        };
        let r = emit_next_generation(WITH_COMMENTED_CHECKSUMS, &params).expect("emit");
        assert!(r.text.contains(
            "{'openmpi-5.0.7.tar.bz2': '119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd'}"
        ), "{}", r.text);
        // The annotations survive: they are not the tool's to rewrite.
        assert!(
            r.text.contains("    # openmpi-5.0.3.tar.bz2\n"),
            "{}",
            r.text
        );
        assert!(
            r.text
                .contains("    # OpenMPI-5.0.3_fix_hle_make_errors.patch\n"),
            "{}",
            r.text
        );
        // The patch entry below the second comment is still left alone.
        assert!(r.text.contains(
            "{'OpenMPI-5.0.3_fix_hle_make_errors.patch': '881c907a9f5901d5d6af41cd33dffdcecba4a67a9e5123e602542aea57a80895'}"
        ), "{}", r.text);
    }

    #[test]
    fn a_comment_and_a_blank_line_above_a_bare_checksum_are_skipped() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: Some(
                "119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd".into(),
            ),
        };
        let r = emit_next_generation(WITH_COMMENTED_BARE_CHECKSUM, &params).expect("emit");
        assert!(
            r.text
                .contains("'119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd'"),
            "{}",
            r.text
        );
        assert!(
            r.text.contains("    # openmpi-5.0.3.tar.bz2\n"),
            "{}",
            r.text
        );
    }

    /// Params for a 5.0.3 -> 5.0.7 bump carrying a new source hash.
    fn bump_with_checksum() -> EmitParams {
        EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: Some(NEW_SHA.into()),
        }
    }

    const NEW_SHA: &str = "119f2009936a403334d0df3c0d74d5595a32d99497f9b1d41e90019fee2fc2dd";

    #[test]
    fn a_typed_tuple_keeps_its_type_and_takes_the_new_value() {
        let r =
            emit_next_generation(WITH_TYPED_TUPLE_CHECKSUM, &bump_with_checksum()).expect("emit");
        assert!(
            r.text.contains(&format!("('sha256', '{NEW_SHA}')")),
            "{}",
            r.text
        );
        // The type is not a checksum and must survive untouched.
        assert!(!r.text.contains(&format!("('{NEW_SHA}'")), "{}", r.text);
    }

    #[test]
    fn alternative_checksums_collapse_to_the_new_value_with_a_warning() {
        let r =
            emit_next_generation(WITH_ALTERNATIVE_CHECKSUMS, &bump_with_checksum()).expect("emit");
        assert!(r.text.contains(&format!("'{NEW_SHA}'")), "{}", r.text);
        // Neither stale alternative may survive a version bump.
        assert!(!r.text.contains("990582f206b3ab32"), "{}", r.text);
        assert!(!r.text.contains("2f1c0b6b1e6d0e6b"), "{}", r.text);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("alternative checksums")),
            "warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn an_and_list_collapses_to_the_new_value_with_a_warning() {
        let r = emit_next_generation(WITH_AND_LIST_CHECKSUM, &bump_with_checksum()).expect("emit");
        assert!(r.text.contains(&format!("'{NEW_SHA}'")), "{}", r.text);
        assert!(!r.text.contains("d0ca4e0f6ac0b9e0"), "{}", r.text);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("checksums that all had to match")),
            "warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn a_dict_value_that_is_a_typed_tuple_has_only_its_value_replaced() {
        let r = emit_next_generation(WITH_DICT_TYPED_TUPLE_CHECKSUM, &bump_with_checksum())
            .expect("emit");
        assert!(
            r.text.contains(&format!(
                "{{'openmpi-5.0.7.tar.bz2': ('sha256', '{NEW_SHA}')}}"
            )),
            "{}",
            r.text
        );
    }

    #[test]
    fn every_documented_shape_bumps_without_erroring() {
        // The bug this pins: one unsupported shape made `bump` exit non-zero and
        // emit nothing, which leaves a maintainer with no file to review.
        for (name, fixture) in [
            ("dict", WITH_CHECKSUMS),
            ("commented dict", WITH_COMMENTED_CHECKSUMS),
            ("commented bare", WITH_COMMENTED_BARE_CHECKSUM),
            ("typed tuple", WITH_TYPED_TUPLE_CHECKSUM),
            ("alternatives", WITH_ALTERNATIVE_CHECKSUMS),
            ("and-list", WITH_AND_LIST_CHECKSUM),
            ("dict of typed tuple", WITH_DICT_TYPED_TUPLE_CHECKSUM),
        ] {
            let r = emit_next_generation(fixture, &bump_with_checksum());
            assert!(r.is_ok(), "{name} shape failed to emit: {:?}", r.err());
        }
    }

    #[test]
    fn an_unknown_form_is_named_in_the_error() {
        let err = emit_next_generation(WITH_UNKNOWN_CHECKSUM_FORM, &bump_with_checksum())
            .expect_err("a form EasyBuild does not accept must not emit a file");
        let msg = err.to_string();
        // The message has to say what it saw, so the reader can fix the recipe
        // rather than guess at what "unrecognized" meant.
        assert!(msg.contains("CHECKSUM_FROM_ELSEWHERE"), "{msg}");
        assert!(msg.contains("('<type>', '<value>')"), "{msg}");
    }

    #[test]
    fn a_shape_without_a_new_checksum_is_left_alone_and_reported_stale() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        for fixture in [
            WITH_TYPED_TUPLE_CHECKSUM,
            WITH_ALTERNATIVE_CHECKSUMS,
            WITH_AND_LIST_CHECKSUM,
        ] {
            let r = emit_next_generation(fixture, &params).expect("emit");
            assert!(
                r.warnings.iter().any(|w| w.contains("stale")),
                "warnings: {:?}",
                r.warnings
            );
            // Nothing invented: the old shape stays until a real hash arrives.
            assert!(r.text.contains("990582f206b3ab32"), "{}", r.text);
        }
    }

    #[test]
    fn the_kokkos_error_row_recipe_emits_a_file() {
        // The scoreboard's only ERROR row. Emitting at all is the property under
        // test: scoring it against the merged target needs a built binary and
        // the target file, which this test cannot supply.
        let params = EmitParams {
            toolchain: Toolchain {
                name: "GCC".into(),
                version: "15.2.0".into(),
            },
            version: Some("5.1.1".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: Some(NEW_SHA.into()),
        };
        let r = emit_next_generation(KOKKOS_ANNOTATED_BARE_CHECKSUMS, &params).expect("emit");
        assert_eq!(r.filename, "Kokkos-5.1.1-GCC-15.2.0.eb");
        assert!(r.text.contains(&format!("'{NEW_SHA}'")), "{}", r.text);
        // The patch hash below the second annotation is not the source's to take.
        assert!(
            r.text
                .contains("'ef8bb57088970fb765acd00bee6d9a07b52383b5e890feb7679bdff3cb1ad0db'"),
            "{}",
            r.text
        );
        // Annotations are the maintainer's, and survive verbatim.
        assert!(
            r.text.contains("    # Kokkos-4.7.01.tar.gz\n"),
            "{}",
            r.text
        );
        // A version bump still owes a patch-set review.
        assert!(
            r.warnings.iter().any(|w| w.contains("patches")),
            "warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn quoted_tokens_ignores_delimiters_inside_strings() {
        let toks = quoted_tokens("('sha256', 'ab)cd')");
        assert_eq!(toks.len(), 2);
        assert_eq!(
            balanced_span("('sha256', 'ab)cd')".as_bytes(), 0, b'(', b')'),
            19,
            "a paren inside a quoted hash must not close the tuple"
        );
    }

    #[test]
    fn skipping_stops_at_the_first_entry() {
        // Asserting on what the index lands on, rather than on the index,
        // keeps the test about the behaviour instead of about arithmetic.
        fn landed_on(s: &str) -> &str {
            &s[skip_to_first_entry(s.as_bytes(), 0)..]
        }
        assert_eq!(landed_on("{'a': 'b'}"), "{'a': 'b'}");
        assert_eq!(landed_on("  \n  {'a': 'b'}"), "{'a': 'b'}");
        assert_eq!(landed_on("\n  # note\n  'sha'"), "'sha'");
        assert_eq!(landed_on("\n # one\n\n # two\n 'sha'"), "'sha'");
        // A comment running to the end leaves nothing, which reads as empty.
        assert_eq!(landed_on("\n # trailing"), "");
        assert_eq!(landed_on(""), "");
    }

    #[test]
    fn version_bump_without_source_checksum_renames_key_and_warns() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(WITH_CHECKSUMS, &params).expect("emit");
        // Key renamed to the new version, but checksum value stays stale.
        assert!(r.text.contains(
            "{'openmpi-5.0.7.tar.bz2': '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b'}"
        ));
        assert_eq!(r.warnings.len(), 2, "warnings: {:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("checksum")));
        assert!(r.warnings.iter().any(|w| w.contains("patches")));
    }

    #[test]
    fn toolchain_only_bump_leaves_checksums_untouched() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: None,
            dep_versions: HashMap::new(),
            dep_toolchains: HashMap::new(),
            source_checksum: None,
        };
        let r = emit_next_generation(WITH_CHECKSUMS, &params).expect("emit");
        assert!(r.text.contains(
            "{'openmpi-5.0.3.tar.bz2': '990582f206b3ab32e938aa31bbf07c639368e4405dca196fabe7f0f76eeda90b'}"
        ));
        assert!(r
            .text
            .contains("'OpenMPI-5.0.3_fix_hle_make_errors.patch',"));
        assert!(r.warnings.is_empty(), "warnings: {:?}", r.warnings);
    }

    #[test]
    fn rewrite_bare_system_toolchain_version_bump_keeps_system() {
        // nvidia-compilers-style recipe: `toolchain = SYSTEM` (no dict). A
        // version-only bump (target toolchain still SYSTEM) must leave the line
        // untouched rather than fail on the missing dict braces.
        let src = "name = 'nvidia-compilers'\nversion = '25.9'\ntoolchain = SYSTEM\n";
        let sys = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let out = rewrite_toolchain(src, &sys).expect("bare SYSTEM must not error");
        assert!(out.contains("toolchain = SYSTEM"), "got:\n{out}");
    }

    #[test]
    fn rewrite_bare_system_toolchain_promoted_to_real_toolchain() {
        // Retargeting a SYSTEM recipe onto a real toolchain promotes the bare
        // token to a dict in place, preserving a trailing comment.
        let src = "name = 'App'\nversion = '1.0'\ntoolchain = SYSTEM  # bootstrap\n";
        let tc = Toolchain {
            name: "GCCcore".into(),
            version: "14.3.0".into(),
        };
        let out = rewrite_toolchain(src, &tc).expect("promotion must succeed");
        assert!(
            out.contains("toolchain = {'name': 'GCCcore', 'version': '14.3.0'}  # bootstrap"),
            "got:\n{out}"
        );
    }
}
