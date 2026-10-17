//! Upstream-derived lint fixtures under `fixtures/lint_*`.
//!
//! Each fixture has `bad.eb`, `good.eb`, and `expected.json`. Linting
//! `bad.eb` must give exactly the listed (rule, line) pairs for the fixture's
//! rules, with the listed severity and a message that starts with the listed
//! text; `good.eb` must give none of them. Other rules may fire on either file
//! (real merged easyconfigs are not perfect). `fixtures/lint_negative/negative.eb`
//! is a set of must-not-flag cases and must give no findings at all.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eb_stack::{lint_style_with, LintOptions, Severity, StyleFinding};
use serde_json::Value;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn lint_file(path: &Path, base: Option<&str>) -> Vec<StyleFinding> {
    let text = std::fs::read_to_string(path).expect("fixture recipe");
    let name = path.file_name().and_then(|name| name.to_str());
    lint_style_with(
        &text,
        &LintOptions {
            file_name: name,
            base,
        },
    )
}

fn describe(findings: &[StyleFinding]) -> String {
    findings
        .iter()
        .map(|finding| {
            format!(
                "{}:{} {} ({:?}) {}",
                finding.line, finding.column, finding.code, finding.severity, finding.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture_dirs() -> Vec<PathBuf> {
    let mut dirs = std::fs::read_dir(fixtures_root())
        .expect("fixtures dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("expected.json").is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lint_"))
        })
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

#[test]
fn all_nineteen_lint_fixtures_are_present() {
    assert_eq!(fixture_dirs().len(), 19, "{:?}", fixture_dirs());
}

#[test]
fn lint_fixtures_match_expected_json() {
    let mut failures = Vec::new();
    for dir in fixture_dirs() {
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("expected.json")).expect("expected.json"),
        )
        .expect("expected.json parses");
        let rules = expected["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let severity = match expected["severity"].as_str() {
            Some("warning") => Severity::Warning,
            _ => Severity::Error,
        };
        let base_text = expected["base"]
            .as_str()
            .map(|base| std::fs::read_to_string(dir.join(base)).expect("base recipe"));
        let base = base_text.as_deref();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();

        let bad = lint_file(&dir.join("bad.eb"), base);
        let got = bad
            .iter()
            .filter(|finding| rules.contains(&finding.code))
            .map(|finding| (finding.code.clone(), finding.line))
            .collect::<BTreeSet<_>>();
        let want = expected["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .map(|finding| {
                (
                    finding["rule"].as_str().unwrap().to_string(),
                    finding["line"].as_u64().unwrap() as usize,
                )
            })
            .collect::<BTreeSet<_>>();
        if got != want {
            failures.push(format!(
                "{name}/bad.eb: want {want:?}, got {got:?}\n{}",
                describe(&bad)
            ));
            continue;
        }
        for finding in expected["findings"].as_array().unwrap() {
            let rule = finding["rule"].as_str().unwrap();
            let line = finding["line"].as_u64().unwrap() as usize;
            let message = finding["message"].as_str().unwrap();
            let produced = bad
                .iter()
                .find(|item| item.code == rule && item.line == line)
                .expect("matched above");
            if !produced.message.starts_with(message) {
                failures.push(format!(
                    "{name}/bad.eb:{line} {rule}: message {:?} does not start with {message:?}",
                    produced.message
                ));
            }
            if produced.severity != severity {
                failures.push(format!(
                    "{name}/bad.eb:{line} {rule}: severity {:?}, want {severity:?}",
                    produced.severity
                ));
            }
        }

        let good = lint_file(&dir.join("good.eb"), base);
        let leftover = good
            .iter()
            .filter(|finding| rules.contains(&finding.code))
            .cloned()
            .collect::<Vec<_>>();
        if !leftover.is_empty() {
            failures.push(format!("{name}/good.eb:\n{}", describe(&leftover)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn negative_cases_give_no_findings() {
    let findings = lint_file(&fixtures_root().join("lint_negative/negative.eb"), None);
    assert!(findings.is_empty(), "{}", describe(&findings));
}

#[test]
fn template_removed_needs_the_base_file() {
    let dir = fixtures_root().join("lint_template_removed");
    let without_base = lint_file(&dir.join("bad.eb"), None);
    assert!(
        without_base
            .iter()
            .all(|finding| finding.code != "EB_FMT_TEMPLATE_REMOVED"),
        "{}",
        describe(&without_base)
    );
}
