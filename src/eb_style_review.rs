//! Reviewer-style easyconfig checks used by `lint_style`.
//!
//! Kept beside the formatter so mechanical wrapping stays about line length.
//! `line_is_mechanically_fixable` lives in the parent module.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;
use rustpython_parser::ast::{self, Ranged, Visitor};
use rustpython_parser::text_size::TextRange;
use rustpython_parser::Parse;
use serde::{Deserialize, Serialize};

use super::{line_is_mechanically_fixable, StyleError, StyleFinding, EB_MAX_LINE};

/// One easyconfig after [`lint_recipe_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeLintFile {
    /// Path that was linted.
    pub path: PathBuf,
    /// Style findings for this file. Local checks come first; `eb` findings follow.
    pub findings: Vec<StyleFinding>,
}

/// Local style lint plus an optional `eb --check-contrib` merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeLintReport {
    /// One entry per input path, in request order.
    pub files: Vec<RecipeLintFile>,
    /// Whether `eb --check-contrib` ran, and why it did not when it did not.
    pub check_contrib: CheckContribReport,
}

/// Outcome of shelling out to `eb --check-contrib`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckContribReport {
    /// True when the `eb` process started.
    pub ran: bool,
    /// Process exit code when `ran` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Short status. A missing `eb` is reported here and is not a lint failure by itself.
    pub detail: String,
}

/// Lint recipe files and, when `eb` is on `PATH`, merge `eb --check-contrib`.
pub fn lint_recipe_files(paths: &[&Path]) -> Result<RecipeLintReport, StyleError> {
    lint_recipe_files_with(paths, None)
}

/// Like [`lint_recipe_files`], but `eb` selects the EasyBuild program.
///
/// `None` searches `PATH` for `eb`. A path runs that program even when `eb`
/// is also installed, which is how tests inject a fake contribution check.
/// A missing explicit path does not fall back to `PATH`.
pub fn lint_recipe_files_with(
    paths: &[&Path],
    eb: Option<&Path>,
) -> Result<RecipeLintReport, StyleError> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| StyleError::Io(format!("read {}: {error}", path.display())))?;
        files.push(RecipeLintFile {
            path: path.to_path_buf(),
            findings: super::lint_style(&text),
        });
    }
    let (check_contrib, extra) = run_check_contrib(eb, paths);
    for file in &mut files {
        let matched = extra
            .iter()
            .filter(|parsed| contrib_belongs(&file.path, parsed, paths.len()))
            .map(|parsed| parsed.finding.clone())
            .collect::<Vec<_>>();
        merge_findings(&mut file.findings, matched);
    }
    Ok(RecipeLintReport {
        files,
        check_contrib,
    })
}

pub(super) fn extend_style(text: &str, out: &mut Vec<StyleFinding>) {
    out.extend(physical_findings(text));
    out.extend(python_style_findings(text));
    sort_findings(out);
}

fn physical_findings(text: &str) -> Vec<StyleFinding> {
    let mut out = Vec::new();
    let mut last_key = String::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let len = line.chars().count();
        if len > EB_MAX_LINE {
            out.push(StyleFinding::e501(
                line_number,
                len,
                line_is_mechanically_fixable(line),
            ));
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(captures) = PARAM_DEF.captures(line) {
            if let Some(key) = captures.name("key") {
                last_key = key.as_str().to_string();
            }
        }
        if SUPPRESSED_TRAILING.contains(&last_key.as_str()) {
            continue;
        }
        if let Some(column) = trailing_whitespace_column(line) {
            out.push(StyleFinding {
                line: line_number,
                column,
                code: "W299".into(),
                message: "trailing whitespace".into(),
                mechanical: false,
                origin: String::new(),
            });
        }
    }
    out
}

fn trailing_whitespace_column(line: &str) -> Option<usize> {
    let trimmed = line.trim_end_matches([' ', '\t']);
    if trimmed.len() == line.len() {
        None
    } else {
        Some(trimmed.chars().count() + 1)
    }
}

fn python_style_findings(text: &str) -> Vec<StyleFinding> {
    let Ok(suite) = ast::Suite::parse(text, "<easyconfig>") else {
        return Vec::new();
    };
    let mut visitor = StyleVisitor {
        text,
        findings: Vec::new(),
        format_depth: 0,
        suppress_names: 0,
    };
    for statement in suite {
        visitor.visit_stmt(statement);
    }
    visitor.findings
}

struct StyleVisitor<'a> {
    text: &'a str,
    findings: Vec<StyleFinding>,
    format_depth: u32,
    suppress_names: u32,
}

impl Visitor for StyleVisitor<'_> {
    fn visit_expr(&mut self, node: ast::Expr) {
        self.note_joined_and_percent(&node);
        if self.format_depth > 0 {
            if self.suppress_names == 0 {
                if let Some(message) = lowercase_template_message(&node) {
                    let range = node.range();
                    self.push(range, "EB101", message);
                    self.suppress_names += 1;
                    self.generic_visit_expr(node);
                    self.suppress_names -= 1;
                    return;
                }
            }
            if self.suppress_names == 0 {
                if let ast::Expr::Name(name) = &node {
                    let id = name.id.as_str();
                    if is_easybuild_template(id) {
                        let range = name.range;
                        self.push(range, "EB101", template_name_message(id));
                    }
                }
            }
        }
        if matches!(node, ast::Expr::FormattedValue(_)) {
            self.format_depth += 1;
            self.generic_visit_expr(node);
            self.format_depth -= 1;
            return;
        }
        self.generic_visit_expr(node);
    }
}

impl StyleVisitor<'_> {
    fn note_joined_and_percent(&mut self, node: &ast::Expr) {
        if let ast::Expr::JoinedStr(joined) = node {
            let has_placeholder = joined
                .values
                .iter()
                .any(|value| matches!(value, ast::Expr::FormattedValue(_)));
            if !has_placeholder {
                self.push(
                    joined.range,
                    "F541",
                    "f-string is missing placeholders. Drop the f prefix when the string only contains EasyBuild templates such as %(name)s, %(version)s, or %(builddir)s. An f-string needs a Python placeholder, for example f'libexec/7z.{SHLIB_EXT}'".to_string(),
                );
            }
        }
        if let ast::Expr::BinOp(binop) = node {
            if matches!(binop.op, ast::Operator::Mod) && is_string_expr(&binop.left) {
                let message = percent_message(self.text, binop.range, &binop.right);
                self.push(binop.range, "UP031", message);
            }
        }
    }

    fn push(&mut self, range: TextRange, code: &str, message: String) {
        let (line, column) = line_col(self.text, u32::from(range.start()) as usize);
        self.findings.push(StyleFinding {
            line,
            column,
            code: code.to_string(),
            message,
            mechanical: false,
            origin: String::new(),
        });
    }
}

fn template_name_message(id: &str) -> String {
    format!(
        "f-string interpolates EasyBuild template name `{id}`; write %({id})s so EasyBuild can resolve it, including with --try-software-version. An f-string is for a Python constant such as SHLIB_EXT (f'libexec/7z.{{SHLIB_EXT}}')"
    )
}

fn lowercase_template_message(expr: &ast::Expr) -> Option<String> {
    let (receiver, attr) = match expr {
        ast::Expr::Call(call) => {
            let ast::Expr::Attribute(attribute) = call.func.as_ref() else {
                return None;
            };
            if !call.args.is_empty() || !call.keywords.is_empty() {
                return None;
            }
            (attribute.value.as_ref(), attribute.attr.as_str())
        }
        ast::Expr::Attribute(attribute) => (attribute.value.as_ref(), attribute.attr.as_str()),
        _ => return None,
    };
    if attr != "lower" {
        return None;
    }
    let ast::Expr::Name(name) = receiver else {
        return None;
    };
    match name.id.as_str() {
        "name" => Some(
            "f-string lowercases `name` in Python; use the EasyBuild template %(namelower)s"
                .to_string(),
        ),
        "nameletter" => Some(
            "f-string lowercases `nameletter` in Python; use the EasyBuild template %(nameletterlower)s"
                .to_string(),
        ),
        _ => None,
    }
}

fn is_string_expr(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Constant(constant) => matches!(constant.value, ast::Constant::Str(_)),
        ast::Expr::JoinedStr(_) => true,
        ast::Expr::BinOp(binop) if matches!(binop.op, ast::Operator::Add) => {
            is_string_expr(&binop.left) && is_string_expr(&binop.right)
        }
        _ => false,
    }
}

fn percent_message(text: &str, range: TextRange, right: &ast::Expr) -> String {
    let mut names = Vec::new();
    collect_names(right, &mut names);
    names.sort();
    names.dedup();
    let templates = names
        .iter()
        .filter(|name| is_easybuild_template(name))
        .cloned()
        .collect::<Vec<_>>();
    let python_names = names
        .iter()
        .filter(|name| !is_easybuild_template(name))
        .cloned()
        .collect::<Vec<_>>();
    let fragment = source_fragment(text, range);
    let guidance = if templates.is_empty() && python_names.len() == 1 {
        let name = &python_names[0];
        format!("use an f-string with a placeholder, for example f'libexec/7z.{{{name}}}'")
    } else if python_names.is_empty() && !templates.is_empty() {
        let shown = templates.join(", ");
        format!(
            "write EasyBuild templates such as %(name)s, %(version)s, and %(builddir)s for {shown}. An f-string is for a Python constant such as SHLIB_EXT, and it is not a substitute for those templates"
        )
    } else {
        "use an f-string for a Python constant or local name (f'libexec/7z.{SHLIB_EXT}') and EasyBuild %(name)s / %(version)s / %(builddir)s templates for values EasyBuild resolves".to_string()
    };
    format!("leftover printf-style formatting `{fragment}`; {guidance}")
}

fn collect_names(expr: &ast::Expr, names: &mut Vec<String>) {
    match expr {
        ast::Expr::Name(name) => names.push(name.id.as_str().to_string()),
        ast::Expr::Tuple(tuple) => {
            for element in &tuple.elts {
                collect_names(element, names);
            }
        }
        ast::Expr::List(list) => {
            for element in &list.elts {
                collect_names(element, names);
            }
        }
        ast::Expr::Dict(dict) => {
            for key in dict.keys.iter().flatten() {
                collect_names(key, names);
            }
            for value in &dict.values {
                collect_names(value, names);
            }
        }
        ast::Expr::Call(call) => {
            collect_names(&call.func, names);
            for argument in &call.args {
                collect_names(argument, names);
            }
            for keyword in &call.keywords {
                collect_names(&keyword.value, names);
            }
        }
        ast::Expr::Attribute(attribute) => collect_names(&attribute.value, names),
        ast::Expr::BinOp(binop) => {
            collect_names(&binop.left, names);
            collect_names(&binop.right, names);
        }
        ast::Expr::JoinedStr(joined) => {
            for value in &joined.values {
                collect_names(value, names);
            }
        }
        ast::Expr::FormattedValue(value) => {
            collect_names(&value.value, names);
            if let Some(spec) = &value.format_spec {
                collect_names(spec, names);
            }
        }
        ast::Expr::Starred(starred) => collect_names(&starred.value, names),
        _ => {}
    }
}

fn source_fragment(text: &str, range: TextRange) -> String {
    let start = u32::from(range.start()) as usize;
    let end = u32::from(range.end()) as usize;
    let raw = text.get(start..end).unwrap_or("");
    let flat: String = raw
        .chars()
        .map(|character| if character == '\n' { ' ' } else { character })
        .collect();
    if flat.chars().count() > 90 {
        let mut trimmed: String = flat.chars().take(87).collect();
        trimmed.push_str("...");
        trimmed
    } else {
        flat
    }
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn is_easybuild_template(name: &str) -> bool {
    EASYBUILD_TEMPLATES.contains(name)
}

fn sort_findings(findings: &mut [StyleFinding]) {
    findings.sort_by(|left, right| {
        (left.line, left.column, left.code.as_str()).cmp(&(
            right.line,
            right.column,
            right.code.as_str(),
        ))
    });
}

fn merge_findings(local: &mut Vec<StyleFinding>, extra: Vec<StyleFinding>) {
    for finding in extra {
        let duplicate = local
            .iter()
            .any(|have| have.line == finding.line && have.code == finding.code);
        if !duplicate {
            local.push(finding);
        }
    }
    sort_findings(local);
}

struct ParsedContrib {
    path: Option<String>,
    finding: StyleFinding,
}

fn contrib_belongs(recipe: &Path, parsed: &ParsedContrib, recipe_count: usize) -> bool {
    match parsed.path.as_deref() {
        Some(raw) => {
            let raw_path = Path::new(raw);
            raw_path == recipe
                || recipe.ends_with(raw_path)
                || raw_path.ends_with(recipe)
                || same_file_name(raw_path, recipe)
        }
        None => {
            let name = recipe
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !name.is_empty() && parsed.finding.message.contains(name) {
                true
            } else {
                recipe_count == 1
            }
        }
    }
}

fn same_file_name(left: &Path, right: &Path) -> bool {
    matches!(
        (left.file_name(), right.file_name()),
        (Some(left_name), Some(right_name)) if left_name == right_name
    )
}

fn run_check_contrib(
    eb: Option<&Path>,
    paths: &[&Path],
) -> (CheckContribReport, Vec<ParsedContrib>) {
    if paths.is_empty() {
        return (
            CheckContribReport {
                ran: false,
                exit_code: None,
                detail: "no recipes to check".into(),
            },
            Vec::new(),
        );
    }
    let program = match resolve_eb(eb) {
        Ok(program) => program,
        Err(detail) => {
            return (
                CheckContribReport {
                    ran: false,
                    exit_code: None,
                    detail,
                },
                Vec::new(),
            );
        }
    };
    let output = Command::new(&program)
        .arg("--check-contrib")
        .args(paths)
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return (
                CheckContribReport {
                    ran: false,
                    exit_code: None,
                    detail: format!("failed to run {}: {error}", program.display()),
                },
                Vec::new(),
            );
        }
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let mut parsed = parse_check_contrib_output(&text);
    let exit_code = output.status.code();
    if !output.status.success() && parsed.is_empty() {
        let detail = if text.trim().is_empty() {
            format!(
                "eb --check-contrib exited {} with no output",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".into())
            )
        } else {
            format!("eb --check-contrib failed: {}", truncate_chars(&text, 400))
        };
        parsed.push(ParsedContrib {
            path: None,
            finding: StyleFinding {
                line: 1,
                column: 1,
                code: "EBCHECK".into(),
                message: detail.clone(),
                mechanical: false,
                origin: "eb --check-contrib".into(),
            },
        });
        return (
            CheckContribReport {
                ran: true,
                exit_code,
                detail,
            },
            parsed,
        );
    }
    let detail = if output.status.success() {
        "eb --check-contrib completed".into()
    } else {
        format!(
            "eb --check-contrib exited {}",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".into())
        )
    };
    (
        CheckContribReport {
            ran: true,
            exit_code,
            detail,
        },
        parsed,
    )
}

fn resolve_eb(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(format!("eb program {} is not a file", path.display()));
    }
    which("eb").ok_or_else(|| "eb is not on PATH; local easyconfig style checks still ran".into())
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn parse_check_contrib_output(text: &str) -> Vec<ParsedContrib> {
    let mut parsed = Vec::new();
    let mut fail_path: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("[FAIL]") {
            fail_path = Some(rest.trim().to_string());
            continue;
        }
        if trimmed.starts_with("[PASS]")
            || trimmed.starts_with(">>")
            || trimmed.starts_with("==")
            || trimmed.starts_with("Running ")
            || trimmed.starts_with("Checking ")
        {
            fail_path = None;
            continue;
        }
        if let Some(captures) = PYCODESTYLE_LINE.captures(trimmed) {
            parsed.push(parsed_style_line(&captures));
            continue;
        }
        if let Some(captures) = ERROR_LINE.captures(trimmed) {
            let message = captures
                .get(1)
                .map(|value| value.as_str().trim().to_string())
                .unwrap_or_default();
            if message.is_empty() {
                continue;
            }
            parsed.push(ParsedContrib {
                path: None,
                finding: contrib_finding(1, 1, &message),
            });
            continue;
        }
        if let Some(path) = &fail_path {
            parsed.push(ParsedContrib {
                path: Some(path.clone()),
                finding: contrib_finding(1, 1, trimmed),
            });
        }
    }
    parsed
}

fn parsed_style_line(captures: &regex::Captures<'_>) -> ParsedContrib {
    let raw_path = captures
        .get(1)
        .map(|value| value.as_str().trim().to_string());
    let line = captures
        .get(2)
        .and_then(|value| value.as_str().parse::<usize>().ok())
        .filter(|line| *line > 0)
        .unwrap_or(1);
    let column = captures
        .get(3)
        .and_then(|value| value.as_str().parse::<usize>().ok())
        .filter(|column| *column > 0)
        .unwrap_or(1);
    let code = captures
        .get(4)
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "EBCHECK".into());
    let message = captures
        .get(5)
        .map(|value| value.as_str().trim().to_string())
        .unwrap_or_default();
    ParsedContrib {
        path: raw_path,
        finding: StyleFinding {
            line,
            column,
            code,
            message,
            mechanical: false,
            origin: "eb --check-contrib".into(),
        },
    }
}

fn contrib_finding(line: usize, column: usize, message: &str) -> StyleFinding {
    let code = if message.to_ascii_lowercase().contains("checksum") {
        "EB256"
    } else {
        "EBCHECK"
    };
    StyleFinding {
        line,
        column,
        code: code.into(),
        message: message.to_string(),
        mechanical: false,
        origin: "eb --check-contrib".into(),
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > max {
        out = out.chars().take(max).collect();
        out.push_str("...");
    }
    out
}

const SUPPRESSED_TRAILING: &[&str] = &["citing", "description", "examples"];

static PARAM_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<key>[a-z_]+)\s*=").expect("param regex"));

static PYCODESTYLE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?):(\d+):(\d+):\s*([A-Z]\d+)\s+(.*)$").expect("pycodestyle regex")
});

static ERROR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:ERROR|Error):\s*(.+)$").expect("error regex"));

/// String templates EasyBuild resolves (`%(name)s` and the rest).
///
/// This is the template-name set from `easybuild.framework.easyconfig.templates`
/// (`TEMPLATE_NAMES_*`, dynamic names, and their alternatives). It is not
/// `TEMPLATE_CONSTANTS`: `SHLIB_EXT` and `SOURCELOWER_TAR_GZ` are Python names.
static EASYBUILD_TEMPLATES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "amdgcn_capabilities",
        "amdgcn_cc_semicolon_sep",
        "amdgcn_cc_space_sep",
        "arch",
        "bitbucket_account",
        "build_dir",
        "builddir",
        "cuda_cc_cmake",
        "cuda_cc_comma_sep",
        "cuda_cc_nvhpc",
        "cuda_cc_semicolon_sep",
        "cuda_cc_space_sep",
        "cuda_cc_space_sep_no_period",
        "cuda_compute_capabilities",
        "cuda_int_comma_sep",
        "cuda_int_semicolon_sep",
        "cuda_int_space_sep",
        "cuda_maj_ver",
        "cuda_short_ver",
        "cuda_sm_comma_sep",
        "cuda_sm_space_sep",
        "cuda_ver",
        "cudamajver",
        "cudashortver",
        "cudaver",
        "github_account",
        "install_dir",
        "installdir",
        "java_maj_ver",
        "java_short_ver",
        "java_ver",
        "javamajver",
        "javashortver",
        "javaver",
        "module_name",
        "mpi_cmd_prefix",
        "name",
        "name_letter",
        "name_letter_lower",
        "name_lower",
        "nameletter",
        "nameletterlower",
        "namelower",
        "parallel",
        "perl_maj_ver",
        "perl_short_ver",
        "perl_ver",
        "perlmajver",
        "perlshortver",
        "perlver",
        "py_maj_ver",
        "py_short_ver",
        "py_ver",
        "pymajver",
        "pyshortver",
        "pyver",
        "r_maj_ver",
        "r_short_ver",
        "r_ver",
        "rmajver",
        "rpath_enabled",
        "rshortver",
        "rver",
        "software_commit",
        "start_dir",
        "sysroot",
        "toolchain_name",
        "toolchain_ver",
        "toolchain_version",
        "ver_maj",
        "ver_maj_min",
        "ver_maj_min_patch",
        "ver_min",
        "ver_min_patch",
        "ver_patch",
        "version",
        "version_major",
        "version_major_minor",
        "version_major_minor_patch",
        "version_minor",
        "version_minor_patch",
        "version_patch",
        "version_prefix",
        "version_suffix",
        "versionprefix",
        "versionsuffix",
    ]
    .into_iter()
    .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    fn codes_on(findings: &[StyleFinding], line: usize) -> Vec<&str> {
        findings
            .iter()
            .filter(|finding| finding.line == line)
            .map(|finding| finding.code.as_str())
            .collect()
    }

    #[test]
    fn template_names_are_not_python_constants() {
        assert!(is_easybuild_template("name"));
        assert!(is_easybuild_template("version"));
        assert!(is_easybuild_template("builddir"));
        assert!(is_easybuild_template("namelower"));
        assert!(!is_easybuild_template("SHLIB_EXT"));
        assert!(!is_easybuild_template("SOURCELOWER_TAR_GZ"));
        assert!(!is_easybuild_template("GITHUB_SOURCE"));
        assert!(!is_easybuild_template("local_makefile"));
    }

    #[test]
    fn flags_printf_style_shlib_and_local_names() {
        let source = "\
prebuildopts = \"cp %s makefile.linux &&\" % local_makefile
files = ['libexec/7z.%s' % SHLIB_EXT]
plain = '%(version)s'
extract_cmd = 'tar -xf %s -C staging --strip-components=1'
n = 4
m = n % 2
mixed = f'lib/%(name)s.{SHLIB_EXT}'
copied = f'cp {local_makefile} makefile.linux && '
";
        let findings = super::super::lint_style(source);
        assert_eq!(codes_on(&findings, 1), ["UP031"]);
        assert_eq!(codes_on(&findings, 2), ["UP031"]);
        assert!(
            findings
                .iter()
                .find(|finding| finding.line == 2)
                .expect("shlib")
                .message
                .contains("f'libexec/7z.{SHLIB_EXT}'"),
            "{findings:?}"
        );
        assert!(findings
            .iter()
            .find(|finding| finding.line == 1)
            .expect("local")
            .message
            .contains("{local_makefile}"));
        assert!(
            findings.iter().all(|finding| finding.code == "UP031"),
            "plain templates, literal %s, numeric modulo, and constant f-strings must stay clean: {findings:?}"
        );
    }

    #[test]
    fn flags_placeholderless_fstring_and_template_interpolation() {
        let source = "\
pretestopts = f'export TMPDIR=%(builddir)s/tmp && mkdir -p $TMPDIR && '
paths = f'{name}/{version}'
lower = f'{name.lower()}'
letter = f'{nameletter.lower()}'
";
        let findings = super::super::lint_style(source);
        assert_eq!(codes_on(&findings, 1), ["F541"]);
        assert!(findings[0].message.contains("placeholders"), "{findings:?}");
        assert_eq!(codes_on(&findings, 2), ["EB101", "EB101"]);
        assert!(findings
            .iter()
            .any(|finding| { finding.line == 2 && finding.message.contains("%(name)s") }));
        assert!(findings
            .iter()
            .any(|finding| { finding.line == 2 && finding.message.contains("%(version)s") }));
        assert_eq!(codes_on(&findings, 3), ["EB101"]);
        assert!(findings
            .iter()
            .find(|finding| finding.line == 3)
            .expect("namelower")
            .message
            .contains("%(namelower)s"));
        assert_eq!(codes_on(&findings, 4), ["EB101"]);
        assert!(findings
            .iter()
            .find(|finding| finding.line == 4)
            .expect("nameletter")
            .message
            .contains("%(nameletterlower)s"));
    }

    #[test]
    fn suppresses_trailing_whitespace_on_description_blocks() {
        let source = "description = \"\"\"hello   \nworld   \n\"\"\"\nhomepage = 'https://example.invalid'  \n# comment   \nname = 'x'\n";
        let findings = super::super::lint_style(source);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, "W299");
        assert_eq!(findings[0].line, 4);
    }

    #[test]
    fn proof_fixtures_flag_leftover_percent_and_f541() {
        // The proof summary called this p7zip 17.06. The easyconfigs file in
        // PR #27186 is p7zip-17.05 (pre-review 594648897944).
        let p7zip = include_str!("../fixtures/review_fix/p7zip-17.05-GCCcore-15.2.0.eb");
        let p7zip_findings = super::super::lint_style(p7zip);
        assert_eq!(codes_on(&p7zip_findings, 26), ["UP031"]);
        assert_eq!(codes_on(&p7zip_findings, 43), ["UP031"]);
        assert!(
            p7zip_findings.iter().all(|finding| finding.code == "UP031"),
            "{p7zip_findings:?}"
        );
        assert!(p7zip_findings
            .iter()
            .find(|finding| finding.line == 43)
            .expect("shlib line")
            .message
            .contains("f'libexec/7z.{SHLIB_EXT}'"));

        let libgd = include_str!("../fixtures/review_fix/libgd-2.3.3-GCCcore-15.2.0.eb");
        let libgd_findings = super::super::lint_style(libgd);
        assert_eq!(libgd_findings.len(), 1, "{libgd_findings:?}");
        assert_eq!(libgd_findings[0].line, 31);
        assert_eq!(libgd_findings[0].code, "F541");
        assert!(libgd_findings.iter().all(|finding| finding.line != 35));
    }

    #[cfg(unix)]
    #[test]
    fn check_contrib_merges_findings_and_keeps_local_on_dedup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("eb");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
printf '%s\\n' \"$2:1:121: E501 line too long (200 > 120 characters)\"\n\
printf '%s\\n' \"[FAIL] $2\"\n\
printf '%s\\n' \"Checksums missing for one or more sources/patches in $(basename \"$2\"): found 1 sources + 0 patches vs 0 checksums\"\n\
exit 1\n",
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&script, permissions).expect("chmod");
        let recipe = temp.path().join("long.eb");
        std::fs::write(
            &recipe,
            format!("configopts = '{}'\nname = 'x'\n", "y".repeat(130)),
        )
        .expect("recipe");

        let report = lint_recipe_files_with(&[recipe.as_path()], Some(&script)).expect("lint");
        assert!(report.check_contrib.ran, "{:?}", report.check_contrib);
        assert_eq!(report.check_contrib.exit_code, Some(1));
        let findings = &report.files[0].findings;
        let e501: Vec<_> = findings
            .iter()
            .filter(|finding| finding.code == "E501")
            .collect();
        assert_eq!(e501.len(), 1, "{findings:?}");
        assert!(e501[0].origin.is_empty(), "{:?}", e501[0]);
        let checksum = findings
            .iter()
            .find(|finding| finding.code == "EB256")
            .expect("checksum finding");
        assert_eq!(checksum.origin, "eb --check-contrib");
        assert!(checksum.message.to_ascii_lowercase().contains("checksum"));

        let missing = temp.path().join("missing-eb");
        let skipped =
            lint_recipe_files_with(&[recipe.as_path()], Some(&missing)).expect("missing eb");
        assert!(!skipped.check_contrib.ran, "{:?}", skipped.check_contrib);
        assert!(skipped.check_contrib.detail.contains("not a file"));
        assert!(skipped.files[0]
            .findings
            .iter()
            .any(|finding| finding.code == "E501"));
    }
}
