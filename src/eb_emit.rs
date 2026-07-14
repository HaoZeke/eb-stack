//! Emit next-generation EasyBuild easyconfigs from an existing recipe.
//!
//! Surgical assignment/list rewrites: only `toolchain`, optional application
//! `version`, and named dependency / build-dependency version fields change.
//! All other source bytes stay verbatim.
//!
//! With `--easyconfigs`, dependency versions are filled by:
//! 1. generation **hierarchy consensus** (GCCcore-aware floors), then
//! 2. **resolvo joint co-select** ([`crate::select::resolvo_resolve_dep_versions`])
//!    so multi-dep pins are SAT-feasible together — not independent lookups.

use crate::domain::Toolchain;
use crate::eb_parse::{parse_easyconfig_tree, ParseError};
use crate::hierarchy::{
    hierarchy_for_with_tree, known_hierarchy, resolve_dep_versions_for_specs, HierarchyError,
    SourceDepSpec, ToolchainHierarchy,
};
use crate::select::resolvo_resolve_dep_versions;
use std::collections::HashMap;
use std::path::Path;
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
    #[error("hierarchy: {0}")]
    Hierarchy(#[from] HierarchyError),
    #[error("parse: {0}")]
    Parse(#[from] ParseError),
    #[error("unresolved dependency {0} under target toolchain {1}-{2}{3}")]
    UnresolvedDep(String, String, String, String),
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
pub fn easyconfig_filename(name: &str, version: &str, toolchain: &Toolchain) -> String {
    format!(
        "{}-{}-{}-{}.eb",
        name, version, toolchain.name, toolchain.version
    )
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
    if !params.dep_versions.is_empty() {
        text = rewrite_dep_list_versions(&text, "dependencies", &params.dep_versions)?;
        text = rewrite_dep_list_versions(&text, "builddependencies", &params.dep_versions)?;
    }

    let mut warnings = Vec::new();
    if version_changed {
        if let Some(old_v) = source_version.as_deref() {
            let (new_text, stale) = rewrite_source_checksum(
                &text,
                old_v,
                &app_version,
                params.source_checksum.as_deref(),
            )?;
            text = new_text;
            if stale {
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

    let filename = easyconfig_filename(&name, &app_version, &params.toolchain);
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

/// Options for auto-resolve when emitting the next generation.
#[derive(Debug, Clone, Default)]
pub struct AutoResolveOpts {
    /// When true, unresolved deps keep the source version (with a loud warning).
    /// Default false: unresolved deps fail the auto-bump.
    pub keep_old: bool,
}

/// Auto-resolve dependency / build-dependency versions from an easyconfig universe
/// using the target toolchain generation's sub-toolchain hierarchy, then emit.
///
/// `hand_overrides` win over auto-resolved versions (same keys). Unresolved
/// dependencies **fail** unless [`AutoResolveOpts::keep_old`] is set.
///
/// When `hierarchy` is `None`, uses [`hierarchy_for_with_tree`]: the
/// `hierarchy_fixture` path, else a built-in fixture, else a hierarchy
/// derived from the easyconfig tree itself.
pub fn emit_next_generation_auto(
    source: &str,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
    hand_overrides: &HashMap<String, String>,
    version: Option<String>,
    source_checksum: Option<String>,
) -> Result<EmitResult, EmitError> {
    emit_next_generation_auto_with_opts(
        source,
        target_toolchain,
        easyconfigs_dir,
        hierarchy,
        hierarchy_fixture,
        hand_overrides,
        version,
        source_checksum,
        &AutoResolveOpts::default(),
    )
}

/// Like [`emit_next_generation_auto`] with explicit [`AutoResolveOpts`].
pub fn emit_next_generation_auto_with_opts(
    source: &str,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
    hand_overrides: &HashMap<String, String>,
    version: Option<String>,
    source_checksum: Option<String>,
    opts: &AutoResolveOpts,
) -> Result<EmitResult, EmitError> {
    let (resolved, mut warnings) = resolve_dep_versions_for_source_with_opts(
        source,
        target_toolchain,
        easyconfigs_dir,
        hierarchy,
        hierarchy_fixture,
        opts,
    )?;
    let mut dep_versions = resolved;
    for (k, v) in hand_overrides {
        dep_versions.insert(k.clone(), v.clone());
    }
    let params = EmitParams {
        toolchain: target_toolchain.clone(),
        version,
        dep_versions,
        source_checksum,
    };
    let mut result = emit_next_generation(source, &params)?;
    warnings.append(&mut result.warnings);
    result.warnings = warnings;
    Ok(result)
}

/// Path-based form of [`emit_next_generation_auto`].
pub fn emit_next_generation_auto_from_path(
    source_path: &Path,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
    hand_overrides: &HashMap<String, String>,
    version: Option<String>,
    source_checksum: Option<String>,
) -> Result<EmitResult, EmitError> {
    emit_next_generation_auto_from_path_with_opts(
        source_path,
        target_toolchain,
        easyconfigs_dir,
        hierarchy,
        hierarchy_fixture,
        hand_overrides,
        version,
        source_checksum,
        &AutoResolveOpts::default(),
    )
}

/// Path-based form of [`emit_next_generation_auto_with_opts`].
pub fn emit_next_generation_auto_from_path_with_opts(
    source_path: &Path,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
    hand_overrides: &HashMap<String, String>,
    version: Option<String>,
    source_checksum: Option<String>,
    opts: &AutoResolveOpts,
) -> Result<EmitResult, EmitError> {
    let source = std::fs::read_to_string(source_path)
        .map_err(|e| EmitError::Rewrite(format!("read {}: {}", source_path.display(), e)))?;
    emit_next_generation_auto_with_opts(
        &source,
        target_toolchain,
        easyconfigs_dir,
        hierarchy,
        hierarchy_fixture,
        hand_overrides,
        version,
        source_checksum,
        opts,
    )
}

/// Resolve dep + builddep versions named in `source` against `easyconfigs_dir`
/// under the target generation's hierarchy (strict: error on missing).
pub fn resolve_dep_versions_for_source(
    source: &str,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
) -> Result<HashMap<String, String>, EmitError> {
    let (map, _warn) = resolve_dep_versions_for_source_with_opts(
        source,
        target_toolchain,
        easyconfigs_dir,
        hierarchy,
        hierarchy_fixture,
        &AutoResolveOpts::default(),
    )?;
    Ok(map)
}

/// Resolve with options; returns (version map, warnings) including parse-skip notes
/// and keep-old / versionsuffix pin messages.
pub fn resolve_dep_versions_for_source_with_opts(
    source: &str,
    target_toolchain: &Toolchain,
    easyconfigs_dir: &Path,
    hierarchy: Option<&ToolchainHierarchy>,
    hierarchy_fixture: Option<&Path>,
    opts: &AutoResolveOpts,
) -> Result<(HashMap<String, String>, Vec<String>), EmitError> {
    let specs = dep_specs_from_source(source)?;
    // No deps/builddeps to resolve: skip hierarchy + resolvo entirely (e.g. leaf
    // OpenMPI-style recipes that only need a toolchain rewrite).
    if specs.is_empty() {
        return Ok((
            HashMap::new(),
            vec!["auto-resolve: no dependencies/builddependencies to resolve".into()],
        ));
    }
    let tree = parse_easyconfig_tree(easyconfigs_dir)?;
    let mut warnings = Vec::new();
    let owned;
    let h = match hierarchy {
        Some(h) => h,
        None => {
            // Fixture path, else built-in, else derive the generation's
            // hierarchy from the robot tree itself, so a brand-new generation
            // (the annual-bump case) needs no shipped fixture.
            owned = hierarchy_for_with_tree(target_toolchain, hierarchy_fixture, &tree.candidates)?;
            if hierarchy_fixture.is_none() && known_hierarchy(target_toolchain).is_none() {
                warnings.push(format!(
                    "hierarchy: derived from robot tree for {}-{} (members: {})",
                    target_toolchain.name,
                    target_toolchain.version,
                    owned.member_labels().join(" < ")
                ));
            }
            &owned
        }
    };
    if !tree.skipped.is_empty() {
        warnings.push(format!(
            "parse: skipped {} unparseable easyconfig(s) under {} ({:.1}% coverage of this tree)",
            tree.skip_count(),
            easyconfigs_dir.display(),
            100.0 * tree.coverage()
        ));
    }
    let (mut map, kept) =
        resolve_dep_versions_for_specs(&specs, &tree.candidates, h, opts.keep_old).map_err(
            |e| match e {
                HierarchyError::MissingDep(name, tn, tv, hint) => {
                    EmitError::UnresolvedDep(name, tn, tv, hint)
                }
                other => EmitError::Hierarchy(other),
            },
        )?;
    for note in kept {
        warnings.push(format!("auto-resolve: {note}"));
    }
    for (name, ver) in &map {
        warnings.push(format!(
            "auto-resolve: {name} → {ver} (hierarchy consensus)"
        ));
    }

    // Joint resolvo SAT over hierarchy-eligible candidates (synthetic root).
    let root_name = assign_string_raw(source, "name").unwrap_or_else(|| "root".into());
    let root_version = assign_string_raw(source, "version").unwrap_or_else(|| "0".into());
    match resolvo_resolve_dep_versions(
        &specs,
        &tree.candidates,
        h,
        target_toolchain,
        &root_name,
        &root_version,
        Some(&map), // hierarchy consensus as exact pins; joint SAT under those
    ) {
        Ok((resolvo_map, engine_note)) => {
            warnings.push(format!("auto-resolve: {engine_note}"));
            for (name, ver) in resolvo_map {
                match map.get(&name) {
                    Some(hver) if hver == &ver => {
                        warnings.push(format!(
                            "auto-resolve: {name} → {ver} (resolvo joint agrees with hierarchy)"
                        ));
                    }
                    Some(hver) => {
                        // Pins should prevent this; keep hierarchy (generation-native).
                        warnings.push(format!(
                            "auto-resolve: {name} resolvo={ver} hierarchy={hver}; keeping hierarchy"
                        ));
                    }
                    None => {
                        warnings.push(format!(
                            "auto-resolve: {name} → {ver} (resolvo joint; hierarchy had no pin)"
                        ));
                        map.insert(name, ver);
                    }
                }
            }
        }
        Err(why) => {
            warnings.push(format!(
                "auto-resolve: resolvo joint skipped ({why}); using hierarchy consensus only"
            ));
        }
    }

    Ok((map, warnings))
}

/// Collect package names from top-level `dependencies` and `builddependencies`.
pub fn dep_names_from_source(source: &str) -> Result<Vec<String>, EmitError> {
    let specs = dep_specs_from_source(source)?;
    Ok(specs.into_iter().map(|s| s.name).collect())
}

/// Split a source line into (code_before_comment, comment_body_without_hash).
/// Comments inside quotes are ignored.
fn split_line_comment(line: &str) -> (&str, Option<&str>) {
    let mut in_s = false;
    let mut in_d = false;
    let b = line.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i] as char;
        if c == '\'' && !in_d {
            in_s = !in_s;
        } else if c == '"' && !in_s {
            in_d = !in_d;
        } else if c == '#' && !in_s && !in_d {
            let code = line[..i].trim_end();
            let comment = line[i + 1..].trim();
            return (code, Some(comment));
        }
        i += 1;
    }
    (line.trim_end(), None)
}

/// True when a trailing easyconfig comment marks the dependency optional.
fn comment_marks_optional(comment: &str) -> bool {
    let c = comment.to_ascii_lowercase();
    // Common EB patterns: "# optional", "# optional dependency", "# needed by X (optional)"
    c.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .any(|tok| tok == "optional")
}

/// Scrape name + version + optional versionsuffix / SYSTEM / optional flag from dep lists.
/// Does not evaluate the full EasyBuild DSL.
pub fn dep_specs_from_source(source: &str) -> Result<Vec<SourceDepSpec>, EmitError> {
    let mut specs = Vec::new();
    // Tuple forms:
    //   ('Name', '1.2.3')
    //   ('Name', '1.2.3', '-suffix')
    //   ('Name', '1.2.3', '', SYSTEM)
    //   ('Name', '1.2.3', '-suffix', SYSTEM)
    //   ('Name', '1.2.3', SYSTEM)
    // Trailing `# optional` on the same line is preserved via line-wise scan.
    let re = regex::Regex::new(
        r#"(?x)
        \(\s*
        (?:'(?P<n1>[^']+)'|"(?P<n2>[^"]+)")
        \s*,\s*
        (?:'(?P<v1>[^']*)'|"(?P<v2>[^"]*)")
        (?P<rest>
            (?:\s*,\s*[^)]*)*
        )
        \s*\)
        "#,
    )
    .map_err(|e| EmitError::Rewrite(e.to_string()))?;

    for key in ["dependencies", "builddependencies"] {
        let Some((list_open_end, list_close_start)) = find_list_span(source, key)? else {
            continue;
        };
        let body = &source[list_open_end..list_close_start];
        // Line-wise so we can attach trailing `# optional` to the dep on that line.
        for line in body.lines() {
            let (code, comment) = split_line_comment(line);
            let optional = comment.is_some_and(comment_marks_optional);
            if code.trim().is_empty() {
                continue;
            }
            for caps in re.captures_iter(code) {
                let name = caps
                    .name("n1")
                    .or_else(|| caps.name("n2"))
                    .map(|m| m.as_str().to_string())
                    .unwrap();
                if name.contains('%') {
                    continue;
                }
                let version = caps
                    .name("v1")
                    .or_else(|| caps.name("v2"))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                let version = version
                    .trim()
                    .trim_start_matches(['>', '<', '=', '!'])
                    .trim()
                    .to_string();
                let rest = caps.name("rest").map(|m| m.as_str()).unwrap_or("");
                let system_toolchain = tuple_rest_is_system(rest);
                let versionsuffix = tuple_rest_versionsuffix(rest);

                specs.push(SourceDepSpec {
                    name,
                    version,
                    versionsuffix,
                    system_toolchain,
                    optional,
                });
            }
        }
    }
    // Prefer first occurrence (runtime over build when same name twice).
    let mut seen = HashMap::new();
    let mut out = Vec::new();
    for s in specs {
        if seen.insert(s.name.clone(), ()).is_none() {
            out.push(s);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `rest` is everything after the version in a dep tuple (leading commas included).
fn tuple_rest_is_system(rest: &str) -> bool {
    // SYSTEM as a bare token (not inside a quoted versionsuffix).
    let mut in_s = false;
    let mut in_d = false;
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '\'' && !in_d {
            in_s = !in_s;
        } else if c == '"' && !in_s {
            in_d = !in_d;
        } else if !in_s && !in_d {
            // Match SYSTEM / system as a word token.
            if rest[i..].starts_with("SYSTEM")
                || rest[i..].to_ascii_lowercase().starts_with("system")
            {
                let tok = if rest[i..].starts_with("SYSTEM") {
                    "SYSTEM"
                } else {
                    // length of "system"
                    "system"
                };
                let end = i + tok.len();
                let before_ok = i == 0
                    || rest.as_bytes()[i - 1].is_ascii_whitespace()
                    || rest.as_bytes()[i - 1] == b',';
                let after_ok = end >= rest.len() || !rest.as_bytes()[end].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    // Avoid matching versionsuffix strings that already closed.
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// First quoted string after the version comma is treated as versionsuffix
/// (empty string → None). Bare SYSTEM is not a versionsuffix.
fn tuple_rest_versionsuffix(rest: &str) -> Option<String> {
    let rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
    if rest.is_empty() {
        return None;
    }
    let bytes = rest.as_bytes();
    let q = bytes[0];
    if q != b'\'' && q != b'"' {
        return None;
    }
    if let Some(end) = rest[1..].find(q as char) {
        let s = &rest[1..1 + end];
        if s.is_empty() {
            return None;
        }
        return Some(s.to_string());
    }
    None
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
        return Err(EmitError::Rewrite(format!("unclosed {key} list")));
    }
    Ok(Some((list_open_end, i)))
}

/// Whether `key = [ ... ]` exists in `src` and has at least one non-whitespace
/// element (used to decide whether a "review the patch set" warning applies).
fn list_is_nonempty(src: &str, key: &str) -> bool {
    matches!(find_list_span(src, key), Ok(Some((s, e))) if !src[s..e].trim().is_empty())
}

/// Rewrite version strings inside `key = [ ... ]` for named deps present in `overrides`.
fn rewrite_dep_list_versions(
    src: &str,
    key: &str,
    overrides: &HashMap<String, String>,
) -> Result<String, EmitError> {
    let Some((list_open_end, list_close_start)) = find_list_span(src, key)? else {
        // No such list — nothing to rewrite (not an error).
        return Ok(src.to_string());
    };
    let body = &src[list_open_end..list_close_start];
    let new_body = rewrite_dep_tuples_in_body(body, overrides)?;
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
fn rewrite_source_checksum(
    src: &str,
    old_version: &str,
    new_version: &str,
    new_checksum: Option<&str>,
) -> Result<(String, bool), EmitError> {
    let Some((list_open_end, list_close_start)) = find_list_span(src, "checksums")? else {
        // No checksums list — nothing to rewrite (not an error).
        return Ok((src.to_string(), false));
    };
    let body = &src[list_open_end..list_close_start];
    let body_bytes = body.as_bytes();

    let mut start = 0usize;
    while start < body_bytes.len() && (body_bytes[start] as char).is_whitespace() {
        start += 1;
    }
    if start >= body_bytes.len() {
        // Empty checksums list — nothing to rewrite.
        return Ok((src.to_string(), false));
    }

    let elem_end = if body_bytes[start] == b'{' {
        let mut depth = 1i32;
        let mut j = start + 1;
        while j < body_bytes.len() {
            match body_bytes[j] as char {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        j
    } else if body_bytes[start] == b'\'' || body_bytes[start] == b'"' {
        let q = body_bytes[start];
        let mut j = start + 1;
        while j < body_bytes.len() && body_bytes[j] != q {
            j += 1;
        }
        (j + 1).min(body_bytes.len())
    } else {
        return Err(EmitError::Rewrite(
            "unrecognized source checksum entry form".into(),
        ));
    };

    let elem = &body[start..elem_end];
    let mut stale = false;

    let new_elem = if elem.starts_with('{') {
        // Dict form: first quoted string is the tarball key, second is the sha.
        // No backreferences (regex crate): match single- and double-quoted
        // spans separately, then merge and sort by position.
        let re_sq = regex::Regex::new(r"'[^']*'").map_err(|e| EmitError::Rewrite(e.to_string()))?;
        let re_dq =
            regex::Regex::new(r#""[^"]*""#).map_err(|e| EmitError::Rewrite(e.to_string()))?;
        let mut matches: Vec<regex::Match> =
            re_sq.find_iter(elem).chain(re_dq.find_iter(elem)).collect();
        matches.sort_by_key(|m| m.start());
        if matches.len() < 2 {
            return Err(EmitError::Rewrite(
                "malformed source checksum dict entry".into(),
            ));
        }
        let key_m = matches[0];
        let val_m = matches[1];
        let key_quote = elem.as_bytes()[key_m.start()] as char;
        let key_str = &elem[key_m.start() + 1..key_m.end() - 1];
        let new_key = if key_str.contains(old_version) {
            key_str.replacen(old_version, new_version, 1)
        } else {
            key_str.to_string()
        };

        let mut out = String::with_capacity(elem.len() + 16);
        out.push_str(&elem[..key_m.start()]);
        out.push(key_quote);
        out.push_str(&new_key);
        out.push(key_quote);
        out.push_str(&elem[key_m.end()..val_m.start()]);
        if let Some(sha) = new_checksum {
            let val_quote = elem.as_bytes()[val_m.start()] as char;
            out.push(val_quote);
            out.push_str(sha);
            out.push(val_quote);
        } else {
            out.push_str(&elem[val_m.start()..val_m.end()]);
            stale = true;
        }
        out.push_str(&elem[val_m.end()..]);
        out
    } else {
        // Bare-string checksum form: no filename key to rename.
        if let Some(sha) = new_checksum {
            let q = elem.chars().next().unwrap();
            format!("{q}{sha}{q}")
        } else {
            stale = true;
            elem.to_string()
        }
    };

    let mut out = String::with_capacity(src.len() + 16);
    out.push_str(&src[..list_open_end + start]);
    out.push_str(&new_elem);
    out.push_str(&src[list_open_end + elem_end..]);
    Ok((out, stale))
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
            source_checksum: None,
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

    fn nvhpc(ver: &str) -> Toolchain {
        Toolchain {
            name: "NVHPC".into(),
            version: ver.into(),
        }
    }

    #[test]
    fn version_bump_with_source_checksum_rewrites_source_entry() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
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
    fn version_bump_without_source_checksum_renames_key_and_warns() {
        let params = EmitParams {
            toolchain: nvhpc("25.11-CUDA-12.8.0"),
            version: Some("5.0.7".into()),
            dep_versions: HashMap::new(),
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
    fn dep_names_from_source_finds_deps_and_builddeps() {
        let names = dep_names_from_source(WITH_BUILDDEPS).expect("names");
        assert_eq!(names, vec!["CMake".to_string(), "OpenMPI".to_string()]);
    }

    #[test]
    fn auto_resolve_fails_loudly_on_missing_dep_unless_keep_old() {
        let src = "\
name = 'App'
version = '1.0'
toolchain = {'name': 'foss', 'version': '2023b'}
dependencies = [
    ('Python', '3.11.5'),
    ('MissingThing', '1.0'),
]
";
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("App.eb");
        std::fs::write(&src_path, src).unwrap();
        // Universe with only Python.
        let uni = tmp.path().join("uni");
        std::fs::create_dir_all(&uni).unwrap();
        std::fs::write(
            uni.join("Python-3.12.3-GCCcore-13.3.0.eb"),
            "name = 'Python'\nversion = '3.12.3'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\ndependencies = []\n",
        )
        .unwrap();
        let empty = HashMap::new();
        let tc = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let err = emit_next_generation_auto_from_path(
            &src_path, &tc, &uni, None, None, &empty, None, None,
        )
        .expect_err("must fail without keep_old");
        let msg = err.to_string();
        assert!(
            msg.contains("MissingThing") || msg.contains("unresolved"),
            "{msg}"
        );

        let ok = emit_next_generation_auto_from_path_with_opts(
            &src_path,
            &tc,
            &uni,
            None,
            None,
            &empty,
            None,
            None,
            &AutoResolveOpts { keep_old: true },
        )
        .expect("keep_old allows emit");
        assert!(ok.text.contains("('Python', '3.12.3')"));
        // MissingThing kept at source version (not rewritten away silently with a wrong new ver).
        assert!(ok.text.contains("('MissingThing', '1.0')"));
        assert!(
            ok.warnings.iter().any(|w| w.contains("MissingThing")),
            "warnings: {:?}",
            ok.warnings
        );
    }

    #[test]
    fn dep_names_from_source_skips_easybuild_constants_in_rest_of_file() {
        // Real GROMACS recipes use SOURCELOWER_TAR_GZ; name extraction must
        // not require evaluating that constant.
        let src = "\
name = 'GROMACS'
version = '2024.4'
toolchain = {'name': 'foss', 'version': '2023b'}
sources = [SOURCELOWER_TAR_GZ]
builddependencies = [
    ('CMake', '3.27.6'),
]
dependencies = [
    ('Python', '3.11.5'),
    ('mpi4py', '3.1.5'),
]
";
        let names = dep_names_from_source(src).expect("names");
        assert_eq!(
            names,
            vec![
                "CMake".to_string(),
                "Python".to_string(),
                "mpi4py".to_string()
            ]
        );
    }

    #[test]
    fn dep_specs_recognize_system_toolchain_and_optional_comment() {
        let src = "\
name = 'PhyloPhlAn'
version = '3.1.1'
toolchain = {'name': 'foss', 'version': '2023b'}
dependencies = [
    ('Python', '3.11.5'),
    ('USEARCH', '11.0.667-i86linux32', '', SYSTEM),
    ('ASE', '3.23.0'),  # optional
    ('MDTraj', '1.10.3'),  # optional
]
";
        let specs = dep_specs_from_source(src).expect("specs");
        let by: HashMap<_, _> = specs.iter().map(|s| (s.name.as_str(), s)).collect();
        assert!(!by["Python"].system_toolchain && !by["Python"].optional);
        assert!(
            by["USEARCH"].system_toolchain,
            "USEARCH must be SYSTEM pin: {:?}",
            by["USEARCH"]
        );
        assert_eq!(by["USEARCH"].version, "11.0.667-i86linux32");
        assert!(by["ASE"].optional, "ASE # optional: {:?}", by["ASE"]);
        assert!(by["MDTraj"].optional);
        assert!(!by["ASE"].system_toolchain);
    }

    #[test]
    fn auto_resolve_preserves_system_pin_but_bumps_optional() {
        let src = "\
name = 'PhyloPhlAn'
version = '3.1.1'
toolchain = {'name': 'foss', 'version': '2023b'}
dependencies = [
    ('Python', '3.11.5'),
    ('USEARCH', '11.0.667-i86linux32', '', SYSTEM),
    ('ASE', '3.23.0'),  # optional
]
";
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("PhyloPhlAn.eb");
        std::fs::write(&src_path, src).unwrap();
        let uni = tmp.path().join("uni");
        std::fs::create_dir_all(&uni).unwrap();
        // Only Python in universe — USEARCH must not cause UnresolvedDep.
        std::fs::write(
            uni.join("Python-3.12.3-GCCcore-13.3.0.eb"),
            "name = 'Python'\nversion = '3.12.3'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\ndependencies = []\n",
        )
        .unwrap();
        // In-generation ASE candidate: `# optional` means optional-to-include,
        // not frozen, so it must bump to this generation version.
        std::fs::write(
            uni.join("ASE-3.24.0-foss-2024a.eb"),
            "name = 'ASE'\nversion = '3.24.0'\ntoolchain = {'name': 'foss', 'version': '2024a'}\ndependencies = []\n",
        )
        .unwrap();
        // Out-of-generation decoy must still not be picked (generation-scoped consensus).
        std::fs::write(
            uni.join("ASE-3.25.0-foss-2025a.eb"),
            "name = 'ASE'\nversion = '3.25.0'\ntoolchain = {'name': 'foss', 'version': '2025a'}\ndependencies = []\n",
        )
        .unwrap();
        let empty = HashMap::new();
        let tc = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let r = emit_next_generation_auto_from_path(
            &src_path, &tc, &uni, None, None, &empty, None, None,
        )
        .expect("SYSTEM freeze + optional bump must not hard-fail");
        assert!(r
            .text
            .contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
        assert!(r.text.contains("('Python', '3.12.3')"));
        // SYSTEM pin still frozen at the source version.
        assert!(r
            .text
            .contains("('USEARCH', '11.0.667-i86linux32', '', SYSTEM)"));
        // Optional ASE bumps to the in-generation candidate; the `# optional`
        // comment is preserved verbatim, only the version token changes.
        assert!(
            r.text.contains("('ASE', '3.24.0'),  # optional"),
            "optional ASE must bump to the generation version with comment preserved, got:\n{}",
            r.text
        );
        assert!(
            !r.text.contains("3.25.0"),
            "out-of-generation decoy must not be used, got:\n{}",
            r.text
        );
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("USEARCH") && w.contains("SYSTEM")),
            "warnings: {:?}",
            r.warnings
        );

        // Unresolved optional (no in-hierarchy candidate): soft-keep source pin.
        let uni2 = tmp.path().join("uni2");
        std::fs::create_dir_all(&uni2).unwrap();
        std::fs::write(
            uni2.join("Python-3.12.3-GCCcore-13.3.0.eb"),
            "name = 'Python'\nversion = '3.12.3'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\ndependencies = []\n",
        )
        .unwrap();
        // Only out-of-generation ASE decoy.
        std::fs::write(
            uni2.join("ASE-3.25.0-foss-2025a.eb"),
            "name = 'ASE'\nversion = '3.25.0'\ntoolchain = {'name': 'foss', 'version': '2025a'}\ndependencies = []\n",
        )
        .unwrap();
        let r_soft = emit_next_generation_auto_from_path(
            &src_path, &tc, &uni2, None, None, &empty, None, None,
        )
        .expect("optional unresolved must soft-keep, not hard-fail");
        assert!(
            r_soft.text.contains("('ASE', '3.23.0')"),
            "optional unresolved ASE must soft-keep source, got:\n{}",
            r_soft.text
        );
        assert!(
            r_soft.warnings.iter().any(|w| w.contains("ASE")
                && w.contains("optional")
                && w.contains("keeping source")),
            "warnings: {:?}",
            r_soft.warnings
        );
    }

    #[test]
    fn auto_resolve_optional_mdtraj_style_extras_bump() {
        // MDTraj-style: networkx/PyTables marked # optional but still generation-bump.
        let src = "\
name = 'MDTraj'
version = '1.10.3'
toolchain = {'name': 'foss', 'version': '2023b'}
dependencies = [
    ('Python', '3.11.5'),
    ('networkx', '3.2.1'),  # optional
    ('PyTables', '3.9.2'),  # optional
]
";
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("MDTraj.eb");
        std::fs::write(&src_path, src).unwrap();
        let uni = tmp.path().join("uni");
        std::fs::create_dir_all(&uni).unwrap();
        for (name, ver, tc_n, tc_v) in [
            ("Python", "3.12.3", "GCCcore", "13.3.0"),
            ("networkx", "3.4.2", "foss", "2024a"),
            ("PyTables", "3.10.2", "foss", "2024a"),
        ] {
            std::fs::write(
                uni.join(format!("{name}-{ver}-{tc_n}-{tc_v}.eb")),
                format!(
                    "name = '{name}'\nversion = '{ver}'\ntoolchain = {{'name': '{tc_n}', 'version': '{tc_v}'}}\ndependencies = []\n"
                ),
            )
            .unwrap();
        }
        let empty = HashMap::new();
        let tc = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let r = emit_next_generation_auto_from_path(
            &src_path, &tc, &uni, None, None, &empty, None, None,
        )
        .expect("optional extras must bump");
        assert!(r.text.contains("('networkx', '3.4.2')"), "got:\n{}", r.text);
        assert!(
            r.text.contains("('PyTables', '3.10.2')"),
            "got:\n{}",
            r.text
        );
        assert!(!r.text.contains("3.2.1"));
        assert!(!r.text.contains("3.9.2"));
    }

    #[test]
    fn auto_resolve_cmake_ignores_out_of_generation_gcccore() {
        let src = "\
name = 'FLANN'
version = '1.9.2'
toolchain = {'name': 'foss', 'version': '2023b'}
builddependencies = [
    ('CMake', '3.27.6'),
]
dependencies = []
";
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("FLANN.eb");
        std::fs::write(&src_path, src).unwrap();
        let uni = tmp.path().join("uni");
        std::fs::create_dir_all(&uni).unwrap();
        std::fs::write(
            uni.join("CMake-3.29.3-GCCcore-13.3.0.eb"),
            "name = 'CMake'\nversion = '3.29.3'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\ndependencies = []\n",
        )
        .unwrap();
        std::fs::write(
            uni.join("CMake-3.31.8-GCCcore-14.3.0.eb"),
            "name = 'CMake'\nversion = '3.31.8'\ntoolchain = {'name': 'GCCcore', 'version': '14.3.0'}\ndependencies = []\n",
        )
        .unwrap();
        std::fs::write(
            uni.join("CMake-3.31.8.eb"),
            "name = 'CMake'\nversion = '3.31.8'\ntoolchain = SYSTEM\ndependencies = []\n",
        )
        .unwrap();
        let empty = HashMap::new();
        let tc = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let r = emit_next_generation_auto_from_path(
            &src_path, &tc, &uni, None, None, &empty, None, None,
        )
        .expect("FLANN bump");
        assert!(
            r.text.contains("('CMake', '3.29.3')"),
            "must pick hierarchy-native CMake 3.29.3, got:\n{}",
            r.text
        );
        assert!(!r.text.contains("3.31.8"));
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
