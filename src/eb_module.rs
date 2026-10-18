//! Checks for Python modules that travel with recipes, such as easyblocks.
//!
//! An easyblock is a Python module, not an easyconfig, so the easyconfig
//! rules in [`crate::eb_style`] do not apply to it. The easyblocks
//! repository's CI runs flake8 with the repository's `setup.cfg`
//! (`max-line-length = 120`, flake8-comprehensions), and its test suite
//! rejects `run_cmd` and `run_cmd_qa` (easybuild-easyblocks#3426). This
//! module runs the checks that need no Python interpreter, and merges real
//! flake8 output when `flake8` is on `PATH`.

use crate::eb_style::{python_syntax_error, Severity, StyleFinding};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// flake8's line length when no configuration file sets one.
pub const FLAKE8_DEFAULT_MAX_LINE: usize = 79;

/// The checks on one Python module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleLintFile {
    /// The module as given.
    pub path: String,
    /// The line length E501 used.
    pub max_line_length: usize,
    /// The configuration file that set `max_line_length`, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Every finding, native and flake8.
    pub findings: Vec<StyleFinding>,
    /// True when no finding is an error outside `info`.
    pub clean: bool,
}

/// Whether flake8 ran, and how it ended.
#[derive(Debug, Clone, Serialize)]
pub struct Flake8Report {
    /// True when flake8 was found and started.
    pub ran: bool,
    /// flake8's exit code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Why it did not run, or a short note on its output.
    pub detail: String,
}

/// The `[flake8]` configuration that governs `path`: the nearest `.flake8`,
/// `setup.cfg` or `tox.ini` above it that has a `[flake8]` section, with its
/// `max-line-length` (flake8's default when the section does not set one).
pub fn flake8_config(path: &Path) -> Option<(PathBuf, usize)> {
    let absolute = path.canonicalize().ok()?;
    for dir in absolute.parent()?.ancestors() {
        for name in [".flake8", "setup.cfg", "tox.ini"] {
            let candidate = dir.join(name);
            let Ok(text) = std::fs::read_to_string(&candidate) else {
                continue;
            };
            if let Some(max) = flake8_max_line(&text) {
                return Some((candidate, max));
            }
        }
    }
    None
}

fn flake8_max_line(text: &str) -> Option<usize> {
    let mut in_section = false;
    let mut found = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed[1..trimmed.len() - 1].trim() == "flake8";
            found |= in_section;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim().replace('_', "-") == "max-line-length" {
                if let Ok(max) = value.trim().parse() {
                    return Some(max);
                }
            }
        }
    }
    found.then_some(FLAKE8_DEFAULT_MAX_LINE)
}

fn finding(
    line: usize,
    column: usize,
    code: &str,
    message: String,
    severity: Severity,
) -> StyleFinding {
    StyleFinding {
        line,
        column,
        code: code.into(),
        message,
        mechanical: false,
        origin: String::new(),
        severity,
        info: false,
        in_review: false,
    }
}

static OLD_SUPER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsuper\(\s*[A-Za-z_]\w*\s*,\s*self\s*\)").expect("regex"));

/// The code part of a line: string literal contents blanked out, and
/// everything from a `#` outside a string dropped.
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in line.chars() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
                out.push(ch);
                continue;
            }
            out.push(' ');
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                out.push(ch);
            }
            '#' => break,
            _ => out.push(ch),
        }
    }
    out
}

fn has_word(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Native checks on a module's text: E999 (not valid Python), E501 at
/// `max_line`, W291/W293 (trailing whitespace), W292/W391 (end of file),
/// EBM_RUN_CMD (`run_cmd`/`run_cmd_qa`, gone in EasyBuild 5) and the
/// EBM_OLD_SUPER warning (`super(Class, self)`, easybuild-easyblocks#3681
/// moved every easyblock to `super()`).
pub fn lint_module_text(text: &str, max_line: usize) -> Vec<StyleFinding> {
    let mut out = Vec::new();
    if let Some((line, column, message)) = python_syntax_error(text) {
        out.push(finding(line, column, "E999", message, Severity::Error));
    }
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let number = index + 1;
        let length = line.chars().count();
        if length > max_line && !line.contains("# noqa") {
            out.push(finding(
                number,
                max_line + 1,
                "E501",
                format!("line too long ({length} > {max_line} characters)"),
                Severity::Error,
            ));
        }
        if line.trim().is_empty() {
            if !line.is_empty() {
                out.push(finding(
                    number,
                    1,
                    "W293",
                    "whitespace on a blank line".into(),
                    Severity::Error,
                ));
            }
        } else if line.ends_with([' ', '\t']) {
            out.push(finding(
                number,
                length,
                "W291",
                "trailing whitespace".into(),
                Severity::Error,
            ));
        }
        let code = code_only(line);
        if has_word(&code, "run_cmd") || has_word(&code, "run_cmd_qa") {
            out.push(finding(
                number,
                1,
                "EBM_RUN_CMD",
                "run_cmd and run_cmd_qa were removed in EasyBuild 5; use run_shell_cmd".into(),
                Severity::Error,
            ));
        }
        if OLD_SUPER.is_match(&code) {
            out.push(finding(
                number,
                1,
                "EBM_OLD_SUPER",
                "use super() without arguments, as every easyblock does since EasyBuild 5".into(),
                Severity::Warning,
            ));
        }
    }
    if !text.is_empty() && !text.ends_with('\n') {
        out.push(finding(
            lines.len(),
            1,
            "W292",
            "no newline at end of file".into(),
            Severity::Error,
        ));
    } else if text.ends_with("\n\n") {
        out.push(finding(
            lines.len(),
            1,
            "W391",
            "blank line at end of file".into(),
            Severity::Error,
        ));
    }
    out
}

/// The `def` or `class` a 1-based line sits in, qualified by the classes
/// around it (`EB_Foo.configure_step`). `None` at module level.
pub fn enclosing_scope(text: &str, line: usize) -> Option<String> {
    static DEF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(\s*)(?:async\s+)?(def|class)\s+([A-Za-z_]\w*)").expect("regex")
    });
    let lines: Vec<&str> = text.lines().collect();
    let start = line.checked_sub(1)?.min(lines.len().checked_sub(1)?);
    let indent_of = |text: &str| text.len() - text.trim_start().len();
    let mut names = Vec::new();
    let first = lines[start];
    let mut limit = if first.trim().is_empty() {
        usize::MAX
    } else if let Some(found) = DEF.captures(first) {
        names.push(found[3].to_string());
        indent_of(first)
    } else {
        indent_of(first)
    };
    for current in lines[..start].iter().rev() {
        if limit == 0 {
            break;
        }
        let trimmed = current.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = indent_of(current);
        if indent >= limit {
            continue;
        }
        if let Some(found) = DEF.captures(current) {
            names.push(found[3].to_string());
        }
        // A less indented statement closes the block we were in; only a
        // def or class above it at a smaller indent can still enclose us.
        limit = indent;
    }
    if names.is_empty() {
        return None;
    }
    names.reverse();
    Some(names.join("."))
}

/// Run flake8 on `path`, with `config` when given. Findings carry origin
/// `flake8`.
pub fn run_flake8(path: &Path, config: Option<&Path>) -> (Flake8Report, Vec<StyleFinding>) {
    let program = std::env::var_os("EB_STACK_FLAKE8")
        .map(PathBuf::from)
        .or_else(|| which("flake8"));
    let Some(program) = program else {
        return (
            Flake8Report {
                ran: false,
                exit_code: None,
                detail: "flake8 is not on PATH; the native checks still ran".into(),
            },
            Vec::new(),
        );
    };
    let mut command = Command::new(&program);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    let output = match command.arg(path).stdin(Stdio::null()).output() {
        Ok(output) => output,
        Err(error) => {
            return (
                Flake8Report {
                    ran: false,
                    exit_code: None,
                    detail: format!("failed to run {}: {error}", program.display()),
                },
                Vec::new(),
            )
        }
    };
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let findings = parse_flake8(&text);
    let detail = if output.status.success() || !findings.is_empty() {
        format!("{} finding(s)", findings.len())
    } else {
        format!(
            "flake8 failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(400)
                .collect::<String>()
        )
    };
    (
        Flake8Report {
            ran: true,
            exit_code: output.status.code(),
            detail,
        },
        findings,
    )
}

/// Parse `path:line:col: CODE message` lines.
pub fn parse_flake8(text: &str) -> Vec<StyleFinding> {
    static LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^.*?:(\d+):(\d+):\s+([A-Z]+\d+)\s+(.*)$").expect("regex"));
    text.lines()
        .filter_map(|line| LINE.captures(line))
        .map(|found| {
            let mut item = finding(
                found[1].parse().unwrap_or(1),
                found[2].parse().unwrap_or(1),
                &found[3],
                found[4].to_string(),
                Severity::Error,
            );
            item.origin = "flake8".into();
            item
        })
        .collect()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Lint one module. With `base` (the module before the edits), a finding
/// whose code and line text were already in `base` is `info` (pre-existing)
/// and does not block clean. flake8 findings replace the native finding with
/// the same code on the same line.
pub fn lint_module(
    path: &Path,
    text: &str,
    base: Option<&str>,
    flake8: bool,
) -> (ModuleLintFile, Flake8Report) {
    let config = flake8_config(path);
    let max_line = config
        .as_ref()
        .map_or(FLAKE8_DEFAULT_MAX_LINE, |(_, max)| *max);
    let mut findings = lint_module_text(text, max_line);
    let report = if flake8 {
        let (report, external) = run_flake8(path, config.as_ref().map(|(file, _)| file.as_path()));
        let seen: HashSet<(usize, String)> = external
            .iter()
            .map(|item| (item.line, item.code.clone()))
            .collect();
        findings.retain(|item| !seen.contains(&(item.line, item.code.clone())));
        findings.extend(external);
        report
    } else {
        Flake8Report {
            ran: false,
            exit_code: None,
            detail: "flake8 not requested".into(),
        }
    };
    if let Some(base) = base {
        let base_lines: Vec<&str> = base.lines().collect();
        let before: HashSet<(String, String)> = lint_module_text(base, max_line)
            .into_iter()
            .filter_map(|item| {
                base_lines
                    .get(item.line.wrapping_sub(1))
                    .map(|line| (item.code, line.to_string()))
            })
            .collect();
        let lines: Vec<&str> = text.lines().collect();
        for item in &mut findings {
            if let Some(line) = lines.get(item.line.wrapping_sub(1)) {
                if before.contains(&(item.code.clone(), line.to_string())) {
                    item.info = true;
                }
            }
        }
    }
    findings.sort_by(|a, b| (a.line, &a.code).cmp(&(b.line, &b.code)));
    let clean = !findings
        .iter()
        .any(|item| item.severity == Severity::Error && !item.info);
    (
        ModuleLintFile {
            path: path.display().to_string(),
            max_line_length: max_line,
            config: config.map(|(file, _)| file.display().to_string()),
            findings,
            clean,
        },
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(findings: &[StyleFinding]) -> Vec<&str> {
        findings.iter().map(|item| item.code.as_str()).collect()
    }

    #[test]
    fn module_checks_cover_syntax_length_whitespace_and_removed_api() {
        let text = "import os\n\n\nclass EB_X(object):\n    def f(self):\n        run_cmd('ls')  \n        super(EB_X, self).f()\n        x = 'run_cmd'  # run_cmd in a comment\n";
        let found = lint_module_text(text, 120);
        assert_eq!(
            codes(&found),
            vec!["W291", "EBM_RUN_CMD", "EBM_OLD_SUPER"],
            "{found:?}"
        );
        let long = format!("x = '{}'\n", "a".repeat(130));
        assert_eq!(codes(&lint_module_text(&long, 120)), vec!["E501"]);
        assert!(lint_module_text(&long, 200).is_empty());
        assert_eq!(codes(&lint_module_text("def f(:\n", 120))[0], "E999");
        assert_eq!(codes(&lint_module_text("x = 1", 120)), vec!["W292"]);
        assert_eq!(codes(&lint_module_text("x = 1\n\n", 120)), vec!["W391"]);
        assert!(lint_module_text("y = run_shell_cmd('ls')\n", 120).is_empty());
    }

    #[test]
    fn flake8_section_sets_the_line_length() {
        assert_eq!(
            flake8_max_line("[flake8]\nmax-line-length = 120\n"),
            Some(120)
        );
        assert_eq!(
            flake8_max_line("[flake8]\nmax_line_length=100\n"),
            Some(100)
        );
        assert_eq!(
            flake8_max_line("[flake8]\nextend-select = C4\n"),
            Some(FLAKE8_DEFAULT_MAX_LINE)
        );
        assert_eq!(flake8_max_line("[metadata]\nname = x\n"), None);
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("setup.cfg"),
            "[flake8]\nmax-line-length = 120\n",
        )
        .expect("cfg");
        std::fs::create_dir_all(dir.path().join("pkg/sub")).expect("dirs");
        let module = dir.path().join("pkg/sub/m.py");
        std::fs::write(&module, "x = 1\n").expect("module");
        let (file, max) = flake8_config(&module).expect("config");
        assert_eq!(max, 120);
        assert!(file.ends_with("setup.cfg"));
    }

    #[test]
    fn preexisting_findings_are_info_against_a_base() {
        let dir = tempfile::tempdir().expect("tempdir");
        let module = dir.path().join("m.py");
        let base = "a = 1  \nb = 2\n";
        let now = "a = 1  \nb = 3  \n";
        std::fs::write(&module, now).expect("write");
        let (file, _) = lint_module(&module, now, Some(base), false);
        let w291: Vec<_> = file
            .findings
            .iter()
            .filter(|item| item.code == "W291")
            .collect();
        assert_eq!(w291.len(), 2);
        assert!(w291[0].info && !w291[1].info, "{w291:?}");
        assert!(!file.clean);
    }

    #[test]
    fn flake8_output_is_parsed() {
        let found = parse_flake8(
            "easybuild/easyblocks/x/x.py:12:5: F401 'os' imported but unused\nnoise\n",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].line, found[0].column, found[0].code.as_str()),
            (12, 5, "F401")
        );
        assert_eq!(found[0].origin, "flake8");
    }

    #[test]
    fn enclosing_scope_names_the_method_and_its_class() {
        let text = "import os\n\nclass EB_X(Base):\n    \"\"\"doc\"\"\"\n\n    def configure_step(self):\n        a = 1\n\n        b = 2\n\n    def test_step(self):\n        pass\n\nTOP = 1\n";
        assert_eq!(
            enclosing_scope(text, 7).as_deref(),
            Some("EB_X.configure_step")
        );
        assert_eq!(
            enclosing_scope(text, 9).as_deref(),
            Some("EB_X.configure_step")
        );
        assert_eq!(enclosing_scope(text, 11).as_deref(), Some("EB_X.test_step"));
        assert_eq!(enclosing_scope(text, 4).as_deref(), Some("EB_X"));
        assert_eq!(enclosing_scope(text, 1), None);
        assert_eq!(enclosing_scope(text, 14), None);
    }
}
