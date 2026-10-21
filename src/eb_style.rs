//! EasyBuild easyconfig style checks and mechanical line wrapping.
//!
//! [`lint_style`] reports what upstream enforces, without needing `eb`:
//!
//! - E501 line length (120), the pycodestyle whitespace codes that
//!   `eb --check-contrib` turns on (E225, E226, E227, E228, E231), and E999
//!   when the file is not valid Python;
//! - the easyconfigs CI lint, `flake8 --select F,W605,W291 --filename '*.eb'`
//!   (F541, W605, W291);
//! - the easyconfigs test-suite checks and reviewer conventions as `EB_*`
//!   rules (`python -s`, `sanity_check_paths` shape, local variable naming,
//!   redefined parameters, `%`/`+` formatting, templates versus f-strings).
//!
//! Each finding carries a [`Severity`]. Errors are what CI or the test suite
//! fails on; warnings are review conventions. When `eb` is on `PATH`,
//! [`lint_recipe_files`] also runs `eb --check-contrib` and merges its
//! findings, but the local rules do not depend on it (it never reports F541).
//!
//! Product content (which `-D` flags) is residual judgment. **Wrapping** long
//! string assignments so each physical line is ≤120 is mechanical and must not
//! burn residual-agent turns. The formatter rewrites long single-quoted or
//! double-quoted assignments such as `key = '…'` and `key += "…"` into
//! parenthesized adjacent string literals. It also wraps long `#` comment
//! lines using the patterns maintainers use by hand. It does not rewrite
//! f-strings or `%` formatting.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[path = "eb_style_diffscope.rs"]
mod diffscope;
#[path = "eb_style_review.rs"]
mod review;
#[path = "eb_style_rules.rs"]
mod rules;
#[path = "eb_style_vocab.rs"]
mod vocab;
pub use diffscope::{
    diff_scope_findings, hunks, review_item_findings, review_item_status, Hunk, ReviewItem,
    ReviewItemStatus,
};
pub use review::{
    lint_recipe_files, lint_recipe_files_against, lint_recipe_files_scoped, lint_recipe_files_with,
    CheckContribReport, RecipeLintFile, RecipeLintReport, RecipeScope,
};
pub use rules::{python_syntax_error, LintOptions};

/// EasyBuild / pycodestyle easyconfig max line length.
pub const EB_MAX_LINE: usize = 120;

/// How much a finding matters upstream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// CI, the easyconfigs test suite, or `eb --check-contrib` fails on it.
    #[default]
    Error,
    /// A review convention. Fix it in lines you touch; it does not fail CI.
    Warning,
}

/// Severity of a finding code. Unknown codes (including `eb` codes) are errors.
pub fn severity_for(code: &str) -> Severity {
    match code {
        "EB_FMT_PERCENT"
        | "EB_FMT_CONCAT"
        | "EB_FMT_TEMPLATE_VAR"
        | "EB_FMT_TEMPLATE_REMOVED"
        | "EB_SHLIB_HARDCODED"
        | "EB_DEP_DUPLICATE"
        | "EB_SANITY_PATHS_REMOVED"
        | "EB_DIFF_OUTSIDE_REVIEW"
        | "EB_DIFF_SOURCES_CHANGED"
        | "EB_REVIEW_ITEM_OPEN"
        | "EB_SANITY_CMD_BUILDDIR"
        | "EB_SANITY_CMD_CD_INSTALLDIR" => Severity::Warning,
        _ => Severity::Error,
    }
}

/// True for the string-formatting family (`EB_FMT_*`: `'%s' % x`,
/// `'...' + SHLIB_EXT`, missing `f`, derived or removed templates). These are
/// what style reviews ask for, so a leftover one means the fix is not done
/// even though CI would not fail on it.
pub fn is_formatting_code(code: &str) -> bool {
    code.starts_with("EB_FMT_")
}

/// Review-convention warnings that block clean when they are on a review
/// line, under `review_scope: "file"`, or on a line that was edited.
pub const REVIEW_COUNTED: &[&str] = &[
    "EB_SHLIB_HARDCODED",
    "EB_SANITY_CMD_BUILDDIR",
    "EB_SANITY_CMD_CD_INSTALLDIR",
];

/// Valid top-level easyconfig parameter names for one recipe.
///
/// This is the generic EasyBuild vocabulary, optionally extended by
/// generic-easyblock-specific keys when the recipe's easyblock is known.
pub fn valid_easyconfig_parameters(easyblock: Option<&str>) -> BTreeSet<String> {
    let mut valid = vocab::DEFAULT_PARAMS
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    if let Some(block) = easyblock {
        if let Some((_, extras)) = vocab::GENERIC_EASYBLOCK_EXTRAS
            .iter()
            .find(|(name, _)| *name == block)
        {
            valid.extend(extras.iter().map(|name| (*name).to_string()));
        }
    }
    valid
}

/// True when a finding keeps a recipe from being clean: any error, and any
/// finding of the formatting family whatever its severity, unless it is
/// outside the review scope (`info`). An error is `info` only when the MCP
/// lint found the same code on the same line text in the file before the
/// edits (pre-existing) and no review line or file-wide scope covers it;
/// `error_free` still reports it.
pub fn blocks_clean(finding: &StyleFinding) -> bool {
    (finding.severity == Severity::Error && !finding.info)
        || (is_formatting_code(&finding.code) && !finding.info)
        || (finding.in_review && REVIEW_COUNTED.contains(&finding.code.as_str()))
        || (finding.code == "EB_DIFF_SOURCES_CHANGED" && !finding.info)
        || (finding.code == "EB_REVIEW_ITEM_OPEN" && !finding.info)
}

/// True when no finding blocks clean: no error, no in-scope `EB_FMT_*`
/// finding, and no `EB_SHLIB_HARDCODED` on a review line. Other warnings
/// (`EB_SHLIB_HARDCODED` elsewhere, `EB_DEP_DUPLICATE`, ...) do not.
pub fn findings_clean(findings: &[StyleFinding]) -> bool {
    !findings.iter().any(blocks_clean)
}

/// True when no finding is an error (what upstream CI would fail on).
pub fn findings_error_free(findings: &[StyleFinding]) -> bool {
    findings
        .iter()
        .all(|finding| finding.severity != Severity::Error)
}

/// One style finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleFinding {
    /// 1-based line number.
    pub line: usize,
    /// 1-based column. For E501 this is [`EB_MAX_LINE`] + 1.
    pub column: usize,
    /// Finding code (`E501`, `W291`, `F541`, `EB_FMT_PERCENT`, ..., or an `eb` code).
    pub code: String,
    /// Human-readable description of the finding.
    pub message: String,
    /// True when `format_style` can rewrite this line deterministically.
    pub mechanical: bool,
    /// Empty for eb-stack's own checks. `eb --check-contrib` when merged.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// `error` when CI or the test suite fails on it, `warning` for review conventions.
    #[serde(default)]
    pub severity: Severity,
    /// True for an `EB_FMT_*` finding outside the review scope: a line that
    /// is unchanged from the base, or (without a base) an untouched
    /// `%%(...)s`-escaped `%` entry; or for an error that was already in the
    /// file before the edits (pre-existing, see [`blocks_clean`]). Reported
    /// for information; it does not make the file unclean.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub info: bool,
    /// True when the finding is on a line in `review_lines`, or (for
    /// `EB_SHLIB_HARDCODED`) when the review scope is the whole file or the
    /// line differs from the base. An `EB_SHLIB_HARDCODED` there is what the
    /// reviewer asked to change, so it blocks clean (proof v5 and v6: a
    /// hardcoded `.so` on a commented line was left because the warning
    /// "needs a change only if the review asks for one", and in v6 because
    /// `review_scope: "file"` without `review_lines` never counted it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_review: bool,
}

/// Result of a format pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatStyleResult {
    /// The formatted source.
    pub text: String,
    /// Number of physical lines that were rewritten.
    pub lines_rewritten: usize,
    /// Style findings still present after format.
    ///
    /// Wrapping only clears mechanical E501 lines. Every other finding, and
    /// long lines the formatter cannot rewrite, stay here.
    pub remaining: Vec<StyleFinding>,
}

#[derive(Debug, Error)]
/// Why a style pass could not run.
pub enum StyleError {
    #[error("io: {0}")]
    /// The file could not be read or written.
    Io(String),
}

impl StyleFinding {
    /// A long-line finding for `line`, which is `len` characters.
    pub fn e501(line: usize, len: usize, mechanical: bool) -> Self {
        Self {
            line,
            column: EB_MAX_LINE + 1,
            code: "E501".into(),
            message: format!("line too long ({len} > {EB_MAX_LINE} characters)"),
            mechanical,
            origin: String::new(),
            severity: Severity::Error,
            info: false,
            in_review: false,
        }
    }
}

/// Lint one easyconfig's text.
///
/// Runs every local rule (see the module docs). A file that is not valid
/// Python gets an E999 finding, and the token-based rules still run on it.
/// This function does not invoke `eb`.
pub fn lint_style(text: &str) -> Vec<StyleFinding> {
    lint_style_with(text, &LintOptions::default())
}

/// Like [`lint_style`], with a file name for messages and an optional base
/// version (diff mode, which adds EB_FMT_TEMPLATE_REMOVED).
pub fn lint_style_with(text: &str, options: &LintOptions<'_>) -> Vec<StyleFinding> {
    let mut out = Vec::new();
    review::extend_style(text, options, &mut out);
    out
}

/// True when [`format_style`] rewrites this physical line.
pub fn line_is_mechanically_fixable(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with('#') {
        return true;
    }
    parse_string_assignment(line).is_some()
        || parse_dictionary_string_list(line).is_some()
        || parse_list_string_item(line).is_some()
}

/// Rewrite fixable long lines; leave other long lines intact (still reported).
pub fn format_style(text: &str) -> FormatStyleResult {
    let ends_with_nl = text.ends_with('\n');
    let mut out_lines: Vec<String> = Vec::new();
    let mut rewritten = 0usize;

    for line in text.lines() {
        let len = line.chars().count();
        if len <= EB_MAX_LINE {
            out_lines.push(line.to_string());
            continue;
        }
        if let Some(fixed) = try_format_line(line) {
            rewritten += 1;
            out_lines.extend(fixed);
        } else {
            out_lines.push(line.to_string());
        }
    }

    let mut text_out = out_lines.join("\n");
    if (ends_with_nl || text.is_empty()) && !text_out.ends_with('\n') {
        text_out.push('\n');
    }
    // Empty input stays empty without forced newline unless original had content.
    if text.is_empty() {
        text_out.clear();
    }

    let mut remaining = lint_style(&text_out);
    // `format_style` also rewrites one indented list item or dict entry. That
    // fragment is not a module, so the E999 from its leading indent is not a
    // finding in the easyconfig it was taken from. A real file has a column-0
    // line, and its syntax errors stay in `remaining`.
    if wholly_indented(&text_out) {
        remaining.retain(|finding| finding.code != "E999");
    }
    FormatStyleResult {
        text: text_out,
        lines_rewritten: rewritten,
        remaining,
    }
}

/// True when every non-blank line is indented, as a list item or dict entry is.
fn wholly_indented(text: &str) -> bool {
    let mut saw_code = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            return false;
        }
        saw_code = true;
    }
    saw_code
}

/// Format a file in place (or write to `out` if set). Returns the format result.
pub fn format_style_file(path: &Path, out: Option<&Path>) -> Result<FormatStyleResult, StyleError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| StyleError::Io(format!("read {}: {e}", path.display())))?;
    let result = format_style(&text);
    let dest = out.unwrap_or(path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| StyleError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(dest, &result.text)
        .map_err(|e| StyleError::Io(format!("write {}: {e}", dest.display())))?;
    Ok(result)
}

fn try_format_line(line: &str) -> Option<Vec<String>> {
    let trimmed_start = line.trim_start();
    if trimmed_start.starts_with('#') {
        let indent_len = line.len() - trimmed_start.len();
        let indent = &line[..indent_len];
        // wrap body after `# `
        let body = trimmed_start.trim_start_matches('#').trim_start();
        let width = EB_MAX_LINE.saturating_sub(indent.chars().count() + 2);
        let wrapped = wrap_words(body, width.max(20));
        return Some(
            wrapped
                .into_iter()
                .map(|w| format!("{indent}# {w}"))
                .collect(),
        );
    }
    if let Some(asg) = parse_string_assignment(line) {
        return Some(format_string_assignment(&asg));
    }
    if let Some(field) = parse_dictionary_string_list(line) {
        return Some(format_dictionary_string_list(&field));
    }
    // List / tuple element: `    'long…',` or `    "long…",`
    if let Some(item) = parse_list_string_item(line) {
        return Some(format_list_string_item(&item));
    }
    None
}

#[derive(Debug)]
struct DictionaryStringList<'a> {
    indent: &'a str,
    key_quote: char,
    key: &'a str,
    value_quote: char,
    value: &'a str,
    trailing_comma: bool,
}

fn parse_dictionary_string_list(line: &str) -> Option<DictionaryStringList<'_>> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = line[indent_len..].trim_end();
    let trailing_comma = rest.ends_with(',');
    let rest = rest.strip_suffix(',').unwrap_or(rest).trim_end();
    let key_quote = rest.chars().next()?;
    if key_quote != '\'' && key_quote != '"' {
        return None;
    }
    let key_body = &rest[key_quote.len_utf8()..];
    let key_end = key_body.find(key_quote)?;
    let key = &key_body[..key_end];
    if key.contains(key_quote) {
        return None;
    }
    let after_key = key_body[key_end + key_quote.len_utf8()..].trim_start();
    let list = after_key.strip_prefix(':')?.trim_start();
    let list = list.strip_prefix('[')?.strip_suffix(']')?.trim();
    let value_quote = list.chars().next()?;
    if value_quote != '\'' && value_quote != '"' || !list.ends_with(value_quote) {
        return None;
    }
    let value = &list[value_quote.len_utf8()..list.len() - value_quote.len_utf8()];
    if contains_unescaped_delimiter(value, value_quote) || value.contains('\n') {
        return None;
    }
    Some(DictionaryStringList {
        indent,
        key_quote,
        key,
        value_quote,
        value,
        trailing_comma,
    })
}

fn format_dictionary_string_list(field: &DictionaryStringList<'_>) -> Vec<String> {
    let mut lines = vec![format!(
        "{}{quote}{}{quote}: [",
        field.indent,
        field.key,
        quote = field.key_quote
    )];
    let item_indent = format!("{}    ", field.indent);
    lines.extend(format_list_string_item(&ListStringItem {
        indent: &item_indent,
        quote: field.value_quote,
        content: field.value,
        trailing_comma: true,
    }));
    lines.push(format!(
        "{}]{}",
        field.indent,
        if field.trailing_comma { "," } else { "" }
    ));
    lines
}

#[derive(Debug)]
struct ListStringItem<'a> {
    indent: &'a str,
    quote: char,
    content: &'a str,
    trailing_comma: bool,
}

fn parse_list_string_item(line: &str) -> Option<ListStringItem<'_>> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = line[indent_len..].trim_end();
    let trailing_comma = rest.ends_with(',');
    let rest = rest.strip_suffix(',').unwrap_or(rest).trim_end();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    if !rest.ends_with(quote) || rest.len() < 2 {
        return None;
    }
    let content = &rest[quote.len_utf8()..rest.len() - quote.len_utf8()];
    if contains_unescaped_delimiter(content, quote) || content.contains('\n') {
        return None;
    }
    Some(ListStringItem {
        indent,
        quote,
        content,
        trailing_comma,
    })
}

fn format_list_string_item(item: &ListStringItem<'_>) -> Vec<String> {
    // (`chunk1 ` + `chunk2`),  — explicit + so the restricted EB parser joins
    let indent = item.indent;
    let q = item.quote;
    let budget = EB_MAX_LINE
        .saturating_sub(indent.chars().count())
        .saturating_sub(4) // ('' )
        .max(16);
    let chunks = split_string_content(item.content, budget);
    let comma = if item.trailing_comma { "," } else { "" };
    if chunks.len() == 1 {
        return vec![format!("{indent}{q}{}{q}{comma}", chunks[0])];
    }
    let mut lines = Vec::new();
    lines.push(format!("{indent}({q}{}{q}", chunks[0]));
    for chunk in &chunks[1..] {
        lines.push(format!("{indent} + {q}{chunk}{q}"));
    }
    // close paren on last line
    if let Some(last) = lines.last_mut() {
        last.push(')');
        last.push_str(comma);
    }
    if lines.iter().any(|l| l.chars().count() > EB_MAX_LINE) {
        // fall back: smaller budget hard-split
        let budget = EB_MAX_LINE
            .saturating_sub(indent.chars().count())
            .saturating_sub(6)
            .max(8);
        let mut rest = item.content;
        let mut out = Vec::new();
        let mut first = true;
        while !rest.is_empty() {
            let take = rest
                .char_indices()
                .take_while(|(i, _)| *i < budget)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(rest.len().min(budget))
                .max(1)
                .min(rest.len());
            let (head, tail) = rest.split_at(take);
            if first {
                out.push(format!("{indent}({q}{head}{q}"));
                first = false;
            } else {
                out.push(format!("{indent} + {q}{head}{q}"));
            }
            rest = tail;
        }
        if let Some(last) = out.last_mut() {
            last.push(')');
            last.push_str(comma);
        }
        return out;
    }
    lines
}

#[derive(Debug)]
struct StringAssignment<'a> {
    indent: &'a str,
    key: &'a str,
    op: &'a str, // "=" or "+="
    quote: char,
    content: &'a str,
}

fn parse_string_assignment(line: &str) -> Option<StringAssignment<'_>> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = line[indent_len..].trim_end();
    // key op quote content quote
    // key: [A-Za-z_][A-Za-z0-9_]*
    let mut chars = rest.char_indices();
    let first = chars.next()?;
    if !(first.1.is_ascii_alphabetic() || first.1 == '_') {
        return None;
    }
    let mut key_end = first.0 + first.1.len_utf8();
    for (i, c) in chars.by_ref() {
        if c.is_ascii_alphanumeric() || c == '_' {
            key_end = i + c.len_utf8();
        } else {
            break;
        }
    }
    let key = &rest[..key_end];
    let after_key = rest[key_end..].trim_start();
    let (op, after_op) = if let Some(r) = after_key.strip_prefix("+=") {
        ("+=", r.trim_start())
    } else {
        ("=", after_key.strip_prefix('=')?.trim_start())
    };
    let quote = after_op.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &after_op[quote.len_utf8()..];
    // content must end with same quote; no unescaped internal same-quote for simplicity
    // (easyconfigs rarely escape quotes in these assignments)
    if !inner.ends_with(quote) {
        return None;
    }
    let content = &inner[..inner.len() - quote.len_utf8()];
    // Reject if content has unescaped newline already (shouldn't on one physical line)
    if content.contains('\n') {
        return None;
    }
    // If the quote appears unescaped inside content, bail (ambiguous)
    if contains_unescaped_delimiter(content, quote) {
        return None;
    }
    Some(StringAssignment {
        indent,
        key,
        op,
        quote,
        content,
    })
}

fn contains_unescaped_delimiter(content: &str, delimiter: char) -> bool {
    let mut backslashes = 0usize;
    for character in content.chars() {
        if character == delimiter && backslashes.is_multiple_of(2) {
            return true;
        }
        if character == '\\' {
            backslashes += 1;
        } else {
            backslashes = 0;
        }
    }
    false
}

fn format_string_assignment(asg: &StringAssignment<'_>) -> Vec<String> {
    // Use `=` then `+=` lines — EasyBuild-common and already handled by the
    // restricted parser (adjacent parenthesized string literals are not).
    //   key = 'chunk1 '
    //   key += 'chunk2 '
    //   key += 'chunk3'
    let indent = asg.indent;
    let prefix_first = format!("{indent}{} {} ", asg.key, asg.op);
    let prefix_cont = format!("{indent}{} += ", asg.key);
    let budget_first = EB_MAX_LINE
        .saturating_sub(prefix_first.chars().count())
        .saturating_sub(2)
        .max(16);
    let budget_cont = EB_MAX_LINE
        .saturating_sub(prefix_cont.chars().count())
        .saturating_sub(2)
        .max(16);
    // First chunk may use first budget; rest use cont budget. Split with min budget.
    let budget = budget_first.min(budget_cont);
    let chunks = split_string_content(asg.content, budget);
    let q = asg.quote;
    let mut lines = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            lines.push(format!("{prefix_first}{q}{chunk}{q}"));
        } else {
            lines.push(format!("{prefix_cont}{q}{chunk}{q}"));
        }
    }
    if lines.iter().any(|l| l.chars().count() > EB_MAX_LINE) {
        return format_string_assignment_hard(asg);
    }
    lines
}

fn format_string_assignment_hard(asg: &StringAssignment<'_>) -> Vec<String> {
    let indent = asg.indent;
    let prefix_first = format!("{indent}{} {} ", asg.key, asg.op);
    let prefix_cont = format!("{indent}{} += ", asg.key);
    let budget = EB_MAX_LINE
        .saturating_sub(prefix_cont.chars().count())
        .saturating_sub(2)
        .max(8);
    let q = asg.quote;
    let mut lines = Vec::new();
    let mut rest = asg.content;
    let mut first = true;
    while !rest.is_empty() {
        let take = preferred_split(rest, budget).unwrap_or_else(|| {
            rest.char_indices()
                .take_while(|(i, _)| *i < budget)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(rest.len().min(budget))
                .max(1)
                .min(rest.len())
        });
        let (head, tail) = rest.split_at(take);
        if first {
            lines.push(format!("{prefix_first}{q}{head}{q}"));
            first = false;
        } else {
            lines.push(format!("{prefix_cont}{q}{head}{q}"));
        }
        rest = tail;
    }
    lines
}

/// Split content preferring shell/flag boundaries, then spaces.
fn split_string_content(content: &str, budget: usize) -> Vec<String> {
    if content.chars().count() <= budget {
        return vec![content.to_string()];
    }
    let mut chunks = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        if rest.chars().count() <= budget {
            chunks.push(rest.to_string());
            break;
        }
        let split = preferred_split(rest, budget).unwrap_or_else(|| {
            rest.char_indices()
                .take_while(|(i, _)| *i < budget)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(budget)
                .max(1)
                .min(rest.len())
        });
        let (head, tail) = rest.split_at(split);
        chunks.push(head.to_string());
        rest = tail;
    }
    chunks
}

fn preferred_split(s: &str, budget: usize) -> Option<usize> {
    // Work in byte indices but only split at char boundaries within budget chars.
    let budget_bytes = s
        .char_indices()
        .take_while(|(i, _)| *i < budget)
        .last()
        .map(|(i, c)| i + c.len_utf8())?;
    let window = &s[..budget_bytes];
    // Prefer longer preferred delimiters first.
    // For flag-like prefixes (` -D`, ` -W`), keep the delimiter with the *next*
    // chunk so we never emit a trailing bare `-D`.
    // Also prefer shell/path breaks (`:`, `$EBROOT…`, `/`) so `$EBROOTFOO`
    // tokens stay contiguous in the file text (review + grep-friendly).
    const PREFS: &[(&str, bool)] = &[
        (" && ", false), // keep && with the left chunk
        (" -D", true),   // next chunk starts with -D…
        (" -W", true),
        (", ", false),
        (" ", false),
        (":", true), // next chunk starts with path segment after :
        ("/", true),
    ];
    for (pref, delim_with_next) in PREFS {
        if let Some(pos) = window.rfind(pref) {
            let end = if *delim_with_next {
                if *pref == " -D" || *pref == " -W" {
                    pos + 1 // space only; next starts at -
                } else {
                    pos + pref.len() // e.g. after `:` or `/`
                }
            } else {
                pos + pref.len()
            };
            // Avoid tiny left chunks (noise).
            if end > 8 && end <= budget_bytes {
                return Some(end);
            }
        }
    }
    // last `$TOKEN` boundary: split before `$` so next chunk keeps full $EBROOT…
    if let Some(pos) = window.rfind('$') {
        if pos > 8 {
            return Some(pos);
        }
    }
    // last whitespace
    if let Some(pos) = window.char_indices().rev().find(|(_, c)| c.is_whitespace()) {
        return Some(pos.0 + pos.1.len_utf8());
    }
    // last non-alnum (avoid mid-identifier hard cuts when possible)
    if let Some((i, _)) = window
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '_')
    {
        if i > 8 {
            return Some(i + 1);
        }
    }
    None
}

fn wrap_words(body: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        if rest.chars().count() <= width {
            lines.push(rest.to_string());
            break;
        }
        let budget_bytes = rest
            .char_indices()
            .take_while(|(i, _)| *i < width)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(width);
        let window = &rest[..budget_bytes.min(rest.len())];
        let split = window
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(budget_bytes.min(rest.len()).max(1));
        let (head, tail) = rest.split_at(split);
        lines.push(head.trim_end().to_string());
        rest = tail.trim_start();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

impl fmt::Display for StyleFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {} {}",
            self.line,
            self.column,
            self.code,
            self.message,
            if self.mechanical {
                "(mechanical)"
            } else {
                "(manual break)"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_empty_ok() {
        assert!(lint_style("").is_empty());
        assert!(lint_style("name = 'x'\n").is_empty());
    }

    #[test]
    fn lint_e501_detects() {
        let long = format!("configopts = '{}'\n", "x".repeat(130));
        let f = lint_style(&long);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "E501");
        assert!(f[0].mechanical);
    }

    #[test]
    fn format_configopts_under_limit() {
        let long_flags = (0..20)
            .map(|i| format!("-Dflag_{i}=ON"))
            .collect::<Vec<_>>()
            .join(" ");
        let src = format!("configopts = '{long_flags}'\n");
        assert!(!lint_style(&src).is_empty());
        let r = format_style(&src);
        assert!(r.lines_rewritten >= 1);
        assert!(r.remaining.is_empty(), "remaining: {:?}", r.remaining);
        assert!(r.text.contains("configopts = "));
        assert!(r.text.contains("configopts += ") || r.lines_rewritten == 1);
        assert!(r.text.contains("-Dflag_0=ON"));
        // re-parse via += chain: joined content equals original
        let joined: String = r
            .text
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                let after = t
                    .strip_prefix("configopts = ")
                    .or_else(|| t.strip_prefix("configopts += "))?;
                let q = after.chars().next()?;
                if q != '\'' && q != '"' {
                    return None;
                }
                Some(after[1..after.len() - 1].to_string())
            })
            .collect();
        assert_eq!(joined, long_flags);
        // Restricted parser must see full joined string.
        let resolved = crate::eb_parse::resolve_easyconfig_str(&format!(
            "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\n{}dependencies = []\n",
            r.text
        ))
        .expect("parse formatted configopts");
        assert_eq!(resolved.configopts.as_deref(), Some(long_flags.as_str()));
    }

    #[test]
    fn format_preconfigopts_plus_equals() {
        let body = format!(
            "export PATH=$EBROOTRUST/bin:$PATH && unset RUSTFLAGS && {}",
            "mkdir -p %(builddir)s/stage && ".repeat(5)
        );
        let src = format!("preconfigopts += '{body}'\n");
        let r = format_style(&src);
        assert!(r.remaining.is_empty(), "{:?}", r.remaining);
        assert!(r.text.contains("preconfigopts += "));
    }

    #[test]
    fn format_keeps_ebroot_tokens_contiguous() {
        let body = "export CMAKE_PREFIX_PATH=%(builddir)s/readcon-stage:$EBROOTPYTORCH:$EBROOTMETATENSOR:$EBROOTMETATENSORMINTORCH:$EBROOTMETATOMICMINTORCH:$EBROOTXTB:$EBROOTCAPNPROTO:$EBROOTQUILL:$EBROOTEIGEN:$EBROOTHIGHWAY:$EBROOTINIH${CMAKE_PREFIX_PATH:+:$CMAKE_PREFIX_PATH} && ";
        let src = format!("preconfigopts += '{body}'\n");
        let r = format_style(&src);
        assert!(r.remaining.is_empty(), "{:?}", r.remaining);
        assert!(
            r.text.contains("$EBROOTMETATENSORMINTORCH"),
            "must not split EBROOT token: {}",
            r.text
        );
        assert!(
            r.text.contains("$EBROOTMETATOMICMINTORCH"),
            "must not split EBROOT token: {}",
            r.text
        );
    }

    #[test]
    fn format_list_item_long_string() {
        let long = "    \"sed 's|^prefix=.*|prefix=%(installdir)s|' %(builddir)s/readcon-stage/lib/pkgconfig/readcon-core.pc > %(installdir)s/lib/pkgconfig/readcon-core.pc\",\n".to_string();
        assert!(long.lines().next().unwrap().chars().count() > EB_MAX_LINE);
        let r = format_style(&long);
        assert!(r.remaining.is_empty(), "{:?}", r.remaining);
        for l in r.text.lines() {
            assert!(l.chars().count() <= EB_MAX_LINE, "{l}");
        }
        assert!(r.text.contains(" + "));
    }

    #[test]
    fn format_list_item_with_escaped_delimiter() {
        let long = "    'command --expression \\\'value with spaces\\\' --input %(builddir)s/a/very/long/path/to/an/input/artifact --output %(installdir)s/a/very/long/path/to/an/output/artifact',\n";
        assert!(long.lines().next().unwrap().chars().count() > EB_MAX_LINE);
        assert!(line_is_mechanically_fixable(long.trim_end()));

        let result = format_style(long);

        assert!(result.remaining.is_empty(), "{:?}", result.remaining);
        assert!(result.text.contains("\\\'value with spaces\\\'"));
        assert!(result.text.contains(" + "));
        assert!(result
            .text
            .lines()
            .all(|line| line.chars().count() <= EB_MAX_LINE));
    }

    #[test]
    fn format_dictionary_single_string_list() {
        let url = format!(
            "https://example.invalid/releases/{}/",
            "0123456789abcdef".repeat(7)
        );
        let source = format!("        'source_urls': ['{url}'],\n");
        assert!(source.lines().next().unwrap().chars().count() > EB_MAX_LINE);
        assert!(line_is_mechanically_fixable(source.trim_end()));

        let result = format_style(&source);

        assert!(result.remaining.is_empty(), "{:?}", result.remaining);
        assert!(result.text.contains("'source_urls': ["));
        assert!(result.text.contains(" + "));
        assert!(result
            .text
            .lines()
            .all(|line| line.chars().count() <= EB_MAX_LINE));
    }

    #[test]
    fn format_comment_wrap() {
        let src = format!("# {}\n", "word ".repeat(40));
        let r = format_style(&src);
        assert!(r.remaining.is_empty());
        assert!(r.text.lines().all(|l| l.chars().count() <= EB_MAX_LINE));
    }

    #[test]
    fn short_lines_unchanged() {
        let src = "name = 'eOn'\nversion = '2.16.0'\n";
        let r = format_style(src);
        assert_eq!(r.lines_rewritten, 0);
        assert_eq!(r.text, src);
    }

    #[test]
    fn qmcpack_configopts_fixture_shape() {
        let line = "configopts = '-DCMAKE_BUILD_TYPE=Release -DQMC_MPI=ON -DQMC_OMP=ON -DQMC_MIXED_PRECISION=OFF -DQMC_COMPLEX=OFF -DBUILD_AFQMC=OFF'\n";
        assert!(
            line.chars().count() > EB_MAX_LINE
                || line.lines().next().unwrap().chars().count() > EB_MAX_LINE
        );
        let r = format_style(line);
        assert!(r.remaining.is_empty(), "{:?}", r.remaining);
        for l in r.text.lines() {
            assert!(
                l.chars().count() <= EB_MAX_LINE,
                "still long ({}): {l}",
                l.chars().count()
            );
        }
    }
}
