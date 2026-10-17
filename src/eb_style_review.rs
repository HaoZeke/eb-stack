//! Recipe-file lint: E501, the local rules, and the optional `eb --check-contrib` merge.
//!
//! Kept beside the formatter so mechanical wrapping stays about line length.
//! `line_is_mechanically_fixable` lives in the parent module, and the rules
//! themselves live in `eb_style_rules.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;
use rustpython_parser::ast;
use rustpython_parser::text_size::TextRange;
use serde::{Deserialize, Serialize};

use super::rules::{self, LintOptions};
use super::{line_is_mechanically_fixable, Severity, StyleError, StyleFinding, EB_MAX_LINE};

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

impl RecipeLintFile {
    /// True when this file has no error and no `EB_FMT_*` finding.
    pub fn clean(&self) -> bool {
        super::findings_clean(&self.findings)
    }

    /// True when this file has no error-severity finding.
    pub fn error_free(&self) -> bool {
        super::findings_error_free(&self.findings)
    }
}

impl RecipeLintReport {
    /// True when no file has an error or an `EB_FMT_*` finding.
    pub fn clean(&self) -> bool {
        self.files.iter().all(RecipeLintFile::clean)
    }

    /// True when no file has an error-severity finding.
    pub fn error_free(&self) -> bool {
        self.files.iter().all(RecipeLintFile::error_free)
    }
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
    lint_recipe_files_inner(paths, None, eb)
}

/// Like [`lint_recipe_files`], comparing each recipe against `base` (the
/// version the change started from), which adds EB_FMT_TEMPLATE_REMOVED.
pub fn lint_recipe_files_against(
    paths: &[&Path],
    base: Option<&Path>,
) -> Result<RecipeLintReport, StyleError> {
    lint_recipe_files_inner(paths, base, None)
}

fn lint_recipe_files_inner(
    paths: &[&Path],
    base: Option<&Path>,
    eb: Option<&Path>,
) -> Result<RecipeLintReport, StyleError> {
    let base_text = match base {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|error| StyleError::Io(format!("read {}: {error}", path.display())))?,
        ),
        None => None,
    };
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| StyleError::Io(format!("read {}: {error}", path.display())))?;
        let file_name = path.file_name().and_then(|name| name.to_str());
        let options = LintOptions {
            file_name,
            base: base_text.as_deref(),
        };
        files.push(RecipeLintFile {
            path: path.to_path_buf(),
            findings: super::lint_style_with(&text, &options),
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

pub(super) fn extend_style(text: &str, options: &LintOptions<'_>, out: &mut Vec<StyleFinding>) {
    out.extend(long_lines(text));
    rules::extend_rules(text, options, out);
    sort_findings(out);
}

fn long_lines(text: &str) -> Vec<StyleFinding> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let len = line.chars().count();
        if len > EB_MAX_LINE {
            out.push(StyleFinding::e501(
                index + 1,
                len,
                line_is_mechanically_fixable(line),
            ));
        }
    }
    out
}

/// What to write instead of `'...' % right`, naming the values on the right.
pub(super) fn percent_guidance(right: &ast::Expr) -> String {
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
    if templates.is_empty() && python_names.len() == 1 {
        let name = &python_names[0];
        format!("make the literal an f-string and put {{{name}}} where the %s is")
    } else if python_names.is_empty() && !templates.is_empty() {
        let shown = templates.join(", ");
        format!(
            "write EasyBuild templates such as %(name)s, %(version)s, and %(builddir)s for {shown}. An f-string is for a Python constant such as SHLIB_EXT, and it is not a substitute for those templates"
        )
    } else {
        "use an f-string for a Python constant or local name (f'lib/libfoo.{SHLIB_EXT}') and EasyBuild %(name)s / %(version)s / %(builddir)s templates for values EasyBuild resolves".to_string()
    }
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

pub(super) fn source_fragment(text: &str, range: TextRange) -> String {
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

/// True when a local finding with `local_code` already reports what
/// `eb --check-contrib` calls `contrib_code`.
fn same_issue(local_code: &str, contrib_code: &str) -> bool {
    local_code == contrib_code
        || (local_code == "W291" && contrib_code == "W299")
        || (local_code == "EB_CHECKSUM_COUNT" && contrib_code == "EB256")
}

fn merge_findings(local: &mut Vec<StyleFinding>, extra: Vec<StyleFinding>) {
    for finding in extra {
        let duplicate = local.iter().any(|have| {
            same_issue(&have.code, &finding.code)
                && (have.line == finding.line || finding.code == "EB256")
        });
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
                severity: Severity::Error,
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
            severity: Severity::Error,
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
        severity: Severity::Error,
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
        assert_eq!(codes_on(&findings, 1), ["EB_FMT_PERCENT"]);
        assert_eq!(codes_on(&findings, 2), ["EB_FMT_PERCENT"]);
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
            findings.iter().all(|finding| finding.code == "EB_FMT_PERCENT"
                && finding.severity == Severity::Warning),
            "plain templates, literal %s, numeric modulo, and constant f-strings must stay clean: {findings:?}"
        );
    }

    #[test]
    fn flags_placeholderless_fstring_and_derived_templates() {
        let source = "\
pretestopts = f'export TMPDIR=%(builddir)s/tmp && mkdir -p $TMPDIR && '
paths = f'{name}/{version}'
lower = f'{name.lower()}'
letter = f'{nameletter.lower()}'
mixed = f'lib/%(name)s.{SHLIB_EXT}'
";
        let findings = super::super::lint_style(source);
        assert_eq!(codes_on(&findings, 1), ["F541"]);
        assert!(findings[0].message.contains("placeholders"), "{findings:?}");
        // Plain top-level {name}/{version} is tolerated (merged precedent).
        assert!(codes_on(&findings, 2).is_empty(), "{findings:?}");
        assert_eq!(codes_on(&findings, 3), ["EB_FMT_TEMPLATE_VAR"]);
        assert!(findings
            .iter()
            .find(|finding| finding.line == 3)
            .expect("namelower")
            .message
            .contains("%(namelower)s"));
        assert_eq!(codes_on(&findings, 4), ["EB_FMT_TEMPLATE_VAR"]);
        assert!(findings
            .iter()
            .find(|finding| finding.line == 4)
            .expect("nameletter")
            .message
            .contains("%(nameletterlower)s"));
        // A template next to a Python constant in one f-string is correct.
        assert!(codes_on(&findings, 5).is_empty(), "{findings:?}");
    }

    #[test]
    fn reports_trailing_whitespace_like_ci_flake8() {
        let source = "description = \"\"\"hello   \nworld   \n\"\"\"\nhomepage = 'https://example.invalid'  \n# comment   \nname = 'x'\n   \n";
        let findings = super::super::lint_style(source);
        let lines: Vec<usize> = findings
            .iter()
            .filter(|finding| finding.code == "W291")
            .map(|finding| finding.line)
            .collect();
        assert_eq!(lines, [1, 2, 4, 5], "{findings:?}");
    }

    #[test]
    fn proof_fixtures_flag_leftover_percent_and_f541() {
        // Pre-review p7zip-17.05 easyconfig frozen under fixtures/review_fix.
        let p7zip = include_str!("../fixtures/review_fix/p7zip-17.05-GCCcore-15.2.0.eb");
        let p7zip_findings = super::super::lint_style(p7zip);
        assert_eq!(codes_on(&p7zip_findings, 26), ["EB_FMT_PERCENT"]);
        assert_eq!(codes_on(&p7zip_findings, 43), ["EB_FMT_PERCENT"]);
        // 'bin/7z.so' in files_to_copy is the build-tree file name, not a
        // library path, so it is not EB_SHLIB_HARDCODED.
        assert!(
            codes_on(&p7zip_findings, 39).is_empty(),
            "{p7zip_findings:?}"
        );
        assert!(
            p7zip_findings
                .iter()
                .all(|finding| finding.code == "EB_FMT_PERCENT"),
            "{p7zip_findings:?}"
        );
        let message_on = |line: usize| {
            p7zip_findings
                .iter()
                .find(|finding| finding.line == line)
                .expect("finding")
                .message
                .clone()
        };
        // Each suggestion is built from its own expression.
        let prebuild = message_on(26);
        assert!(
            prebuild.contains(
                "becomes `f\"cp {local_makefile} makefile.linux && cp makefile.linux makefile.machine &&\"`"
            ),
            "{prebuild}"
        );
        assert!(!prebuild.contains("7z."), "{prebuild}");
        let shlib = message_on(43);
        assert!(
            shlib.contains("becomes `f'libexec/7z.{SHLIB_EXT}'`"),
            "{shlib}"
        );
        assert!(!shlib.contains("local_makefile"), "{shlib}");
        // An EB_FMT finding keeps the file from being clean.
        assert!(!super::super::findings_clean(&p7zip_findings));
        assert!(super::super::findings_error_free(&p7zip_findings));

        let libgd = include_str!("../fixtures/review_fix/libgd-2.3.3-GCCcore-15.2.0.eb");
        let libgd_findings = super::super::lint_style(libgd);
        assert_eq!(libgd_findings.len(), 1, "{libgd_findings:?}");
        assert_eq!(libgd_findings[0].line, 31);
        assert_eq!(libgd_findings[0].code, "F541");
        assert!(libgd_findings.iter().all(|finding| finding.line != 35));
    }

    #[test]
    fn check_contrib_trailing_whitespace_and_checksums_dedup_against_local_rules() {
        let mut local = vec![StyleFinding {
            line: 3,
            column: 10,
            code: "W291".into(),
            message: "trailing whitespace".into(),
            mechanical: false,
            origin: String::new(),
            severity: Severity::Error,
        }];
        let contrib = contrib_finding(3, 10, "W299 trailing whitespace");
        let mut w299 = contrib.clone();
        w299.code = "W299".into();
        merge_findings(&mut local, vec![w299]);
        assert_eq!(local.len(), 1, "{local:?}");
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
