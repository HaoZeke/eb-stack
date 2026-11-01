//! Fail when a build list names software a baseline already provides.
//!
//! A site layer is the software EESSI does not install. `stack solve
//! --build-list-excludes-baseline` writes that list. A later edit can put
//! an EESSI module back on it. `stack provided` reads the list and the
//! baseline (an installed-module listing, a stack lock, or both) and fails
//! when a line's package name is one the baseline has, ignoring version
//! and toolchain. `--allow` keeps a second version the site was asked for.
//!
//! The same pass also rejects a toolchain other than the foss-2026.1
//! hierarchy (GCCcore-15.2.0 below it), a per-line `--hooks` or `--hook`
//! flag, and a name that only a separate family lock covers. Those are
//! errors. Group-restricted names are warnings, and only when
//! `--licensed` or `--licensed-file` names them.

use crate::cli_parse::usage;
use crate::domain::{StackLock, Toolchain};
use crate::easystack::filename_toolchain;
use crate::gap::{names_from_listing, software_name};
use crate::hierarchy::known_hierarchy;
use crate::jenkins::parse_build_list;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Toolchain names of the LLVM family. A foss solve cannot see them.
const LLVM_FAMILY: &[&str] = &["lfoss", "lompi", "lfbf", "llvm-compilers"];

/// How `stack provided` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvidedFormat {
    /// The `provided` / `unparsed` lines, then any further findings, then a count.
    Text,
    /// GitHub Actions workflow commands.
    Github,
    /// One JSON document.
    Json,
}

impl ProvidedFormat {
    /// `text`, `github`, or `json`.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "text" => Ok(Self::Text),
            "github" => Ok(Self::Github),
            "json" => Ok(Self::Json),
            other => Err(usage(format!(
                "--format {other} is not text, github, or json"
            ))),
        }
    }
}

/// Arguments for `stack provided`.
pub struct ProvidedRequest {
    /// Jenkins or path-style build lists. Repeatable.
    pub build_lists: Vec<PathBuf>,
    /// Installed-module listings, `Name/Version` or a bare name. Repeatable.
    pub installed: Vec<PathBuf>,
    /// Stack lock whose package names count as provided.
    pub baseline_lock: Option<PathBuf>,
    /// Package names that may stay on the list. Matched without regard to case.
    ///
    /// This skips a provided hit and a family-lock hit. It does not skip a
    /// wrong toolchain or a `--hooks` flag.
    pub allow: Vec<String>,
    /// Stack locks of families the foss baseline cannot see, such as lfoss.
    /// A name that is only in one of these is not reported as site layer.
    pub family_locks: Vec<PathBuf>,
    /// Extra group-restricted names. Combined with `--licensed-file`.
    pub licensed: Vec<String>,
    /// Name list that replaces passing each group-restricted name as `--licensed`.
    /// Absent, and with no `--licensed`, there is no warning.
    pub licensed_file: Option<PathBuf>,
    /// Print the report as JSON. Wins over `format` when set.
    pub json: bool,
    /// Text, GitHub workflow commands, or JSON.
    pub format: ProvidedFormat,
    /// Exit 1 when a group-restricted warning remains.
    pub strict: bool,
}

/// One build-list line whose package the baseline already provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvidedHit {
    /// Build list the line came from.
    pub list: String,
    /// The line's easyconfig, as written.
    pub file: String,
    /// Package name parsed from that filename.
    pub name: String,
    /// 1-based line in the build list.
    pub line: usize,
}

/// One further finding on a build-list line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineNote {
    /// Build list the line came from.
    pub list: String,
    /// 1-based line in that file.
    pub line: usize,
    /// The line's easyconfig, as written.
    pub file: String,
    /// Package name parsed from that filename.
    pub name: String,
    /// The rest of the message, after the file, name, list, and line.
    pub detail: String,
}

/// One build-list line that names no package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnparsedLine {
    /// Build list the line came from.
    pub list: String,
    /// The line's easyconfig, as written.
    pub file: String,
    /// 1-based line in the build list.
    pub line: usize,
}

/// What `stack provided` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvidedReport {
    /// Lines whose package the baseline provides and `--allow` does not name.
    pub hits: Vec<ProvidedHit>,
    /// `.eb` lines whose filename names no package.
    pub unparsed: Vec<UnparsedLine>,
    /// Lines `--allow` kept off `hits`.
    pub allowed: usize,
    /// Distinct package names the baseline provides.
    pub provided: usize,
    /// Lines whose toolchain is not 2026.1 or GCCcore-15.2.0 below it.
    pub toolchain: Vec<LineNote>,
    /// Lines that set `--hooks` or `--hook`.
    pub hooks: Vec<LineNote>,
    /// Lines whose name belongs to a family lock rather than the site layer.
    pub family: Vec<LineNote>,
    /// Lines that name a group-restricted package. Warnings.
    pub licensed: Vec<LineNote>,
    /// Whether a licensed warning fails the command.
    #[serde(skip)]
    pub strict: bool,
    /// Text for stdout. JSON when the request asked for it.
    #[serde(skip)]
    pub stdout: String,
}

impl ProvidedReport {
    /// The stderr line when the check failed, or `None` when it passed.
    pub fn failure(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.hits.is_empty() {
            let n = self.hits.len();
            parts.push(format!(
                "{n} build-list {} {} software the baseline already provides",
                plural(n, "line", "lines"),
                plural(n, "names", "name"),
            ));
        }
        if !self.unparsed.is_empty() {
            let n = self.unparsed.len();
            parts.push(format!(
                "{n} build-list {} {} no package",
                plural(n, "line", "lines"),
                plural(n, "names", "name"),
            ));
        }
        if !self.toolchain.is_empty() {
            let n = self.toolchain.len();
            parts.push(format!(
                "{n} build-list {} {} a toolchain other than 2026.1 or GCCcore-15.2.0",
                plural(n, "line", "lines"),
                plural(n, "uses", "use"),
            ));
        }
        if !self.hooks.is_empty() {
            let n = self.hooks.len();
            parts.push(format!(
                "{n} build-list {} {} --hooks",
                plural(n, "line", "lines"),
                plural(n, "sets", "set"),
            ));
        }
        if !self.family.is_empty() {
            let n = self.family.len();
            parts.push(format!(
                "{n} build-list {} {} in a family lock",
                plural(n, "line", "lines"),
                plural(n, "belongs", "belong"),
            ));
        }
        if self.strict && !self.licensed.is_empty() {
            let n = self.licensed.len();
            parts.push(format!(
                "{n} build-list {} {} group-restricted software",
                plural(n, "line", "lines"),
                plural(n, "names", "name"),
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Read the lists and compare each line's package name with the baseline.
pub fn run_provided(args: &ProvidedRequest) -> Result<ProvidedReport> {
    if args.build_lists.is_empty() {
        return Err(usage("pass at least one --build-list"));
    }
    if args.installed.is_empty() && args.baseline_lock.is_none() {
        return Err(usage(
            "pass --installed or --baseline-lock so the check knows what the baseline provides",
        ));
    }
    let mut provided = BTreeSet::new();
    for path in &args.installed {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--installed {}: not readable", path.display()))?;
        provided.extend(
            names_from_listing(&text)
                .into_iter()
                .map(|name| name.to_lowercase()),
        );
    }
    if let Some(path) = &args.baseline_lock {
        let lock: StackLock = crate::load_json_file(path)
            .with_context(|| format!("--baseline-lock {}", path.display()))?;
        provided.extend(
            lock.packages
                .iter()
                .map(|package| package.name.to_lowercase()),
        );
    }
    let allow: BTreeSet<String> = args.allow.iter().map(|name| name.to_lowercase()).collect();
    let families = args
        .family_locks
        .iter()
        .map(|path| read_family_lock(path))
        .collect::<Result<Vec<_>>>()?;
    let licensed = read_licensed(args.licensed_file.as_deref(), &args.licensed)?;
    let allowed_toolchains = site_layer_toolchains()?;
    let mut hits = Vec::new();
    let mut unparsed = Vec::new();
    let mut allowed = 0usize;
    let mut toolchain = Vec::new();
    let mut hooks = Vec::new();
    let mut family = Vec::new();
    let mut licensed_notes = Vec::new();
    for path in &args.build_lists {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--build-list {}: not readable", path.display()))?;
        let list = path.display().to_string();
        classify_list(
            &list,
            &text,
            &provided,
            &allow,
            &mut hits,
            &mut unparsed,
            &mut allowed,
        );
        note_extras(
            &list,
            &text,
            &NoteCtx {
                provided: &provided,
                allow: &allow,
                families: &families,
                licensed: &licensed,
                allowed_toolchains: &allowed_toolchains,
            },
            &mut toolchain,
            &mut hooks,
            &mut family,
            &mut licensed_notes,
        );
    }
    let format = if args.json {
        ProvidedFormat::Json
    } else {
        args.format
    };
    let report = ProvidedReport {
        hits,
        unparsed,
        allowed,
        provided: provided.len(),
        toolchain,
        hooks,
        family,
        licensed: licensed_notes,
        strict: args.strict,
        stdout: String::new(),
    };
    let stdout = render(&report, format)?;
    Ok(ProvidedReport { stdout, ..report })
}

struct FamilyLock {
    path: String,
    label: String,
    toolchain_name: String,
    names: BTreeMap<String, String>,
}

fn read_family_lock(path: &Path) -> Result<FamilyLock> {
    if path.is_dir() {
        return Err(usage(format!(
            "--family-lock {} is a directory; pass the lock file",
            path.display()
        )));
    }
    let lock: StackLock =
        crate::load_json_file(path).with_context(|| format!("--family-lock {}", path.display()))?;
    let label = lock
        .generation_label
        .clone()
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| lock.toolchain.label());
    let mut names = BTreeMap::new();
    for package in &lock.packages {
        names
            .entry(package.name.to_lowercase())
            .or_insert_with(|| package.name.clone());
    }
    Ok(FamilyLock {
        path: path.display().to_string(),
        label,
        toolchain_name: lock.toolchain.name,
        names,
    })
}

fn read_licensed(file: Option<&Path>, extra: &[String]) -> Result<BTreeMap<String, String>> {
    let mut names = BTreeMap::new();
    if let Some(path) = file {
        if path.is_dir() {
            return Err(usage(format!(
                "--licensed-file {} is a directory; pass the name list",
                path.display()
            )));
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--licensed-file {}", path.display()))?;
        for name in parse_name_list(&text) {
            names.entry(name.to_lowercase()).or_insert(name);
        }
    }
    for name in extra {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            names
                .entry(trimmed.to_lowercase())
                .or_insert_with(|| trimmed.to_string());
        }
    }
    Ok(names)
}

fn parse_name_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or(line).trim();
            line.split_whitespace()
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect()
}

fn site_layer_toolchains() -> Result<Vec<String>> {
    let parent = Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    };
    let hierarchy = known_hierarchy(&parent).context("foss-2026.1 hierarchy fixture")?;
    Ok(hierarchy
        .members
        .iter()
        .filter(|member| member.name != "system")
        .map(|member| member.label())
        .collect())
}

fn classify_list(
    list: &str,
    text: &str,
    provided: &BTreeSet<String>,
    allow: &BTreeSet<String>,
    hits: &mut Vec<ProvidedHit>,
    unparsed: &mut Vec<UnparsedLine>,
    allowed: &mut usize,
) {
    for entry in entries(text) {
        let Some(name) = software_name(&entry.file) else {
            unparsed.push(UnparsedLine {
                list: list.to_string(),
                file: entry.file,
                line: entry.line,
            });
            continue;
        };
        if !provided.contains(&name.to_lowercase()) {
            continue;
        }
        if allow.contains(&name.to_lowercase()) {
            *allowed += 1;
            continue;
        }
        hits.push(ProvidedHit {
            list: list.to_string(),
            file: entry.file,
            name,
            line: entry.line,
        });
    }
}

struct Entry {
    line: usize,
    file: String,
    flags: Vec<String>,
}

fn entries(text: &str) -> Vec<Entry> {
    text.lines()
        .enumerate()
        .filter_map(|(index, raw)| {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let line = parse_build_list(trimmed).into_iter().next()?;
            Some(Entry {
                line: index + 1,
                file: line.file,
                flags: line.flags,
            })
        })
        .collect()
}

struct NoteCtx<'a> {
    provided: &'a BTreeSet<String>,
    allow: &'a BTreeSet<String>,
    families: &'a [FamilyLock],
    licensed: &'a BTreeMap<String, String>,
    allowed_toolchains: &'a [String],
}

fn note_extras(
    list: &str,
    text: &str,
    ctx: &NoteCtx<'_>,
    toolchain_notes: &mut Vec<LineNote>,
    hook_notes: &mut Vec<LineNote>,
    family_notes: &mut Vec<LineNote>,
    licensed_notes: &mut Vec<LineNote>,
) {
    for entry in entries(text) {
        let Some(name) = software_name(&entry.file) else {
            continue;
        };
        let key = name.to_lowercase();
        if let Some(detail) = toolchain_detail(&entry.file, ctx.families, ctx.allowed_toolchains) {
            toolchain_notes.push(note(list, &entry, &name, detail));
        }
        let hooks = hook_flags(&entry.flags);
        if !hooks.is_empty() {
            hook_notes.push(note(
                list,
                &entry,
                &name,
                format!(
                    "sets={}; which replaces EESSI's hooks",
                    english_join(&hooks)
                ),
            ));
        }
        if !ctx.provided.contains(&key) && !ctx.allow.contains(&key) {
            let hits: Vec<&FamilyLock> = ctx
                .families
                .iter()
                .filter(|family| family.names.contains_key(&key))
                .collect();
            if !hits.is_empty() {
                let described = english_join(
                    &hits
                        .iter()
                        .map(|family| format!("lock={} label={}", family.path, family.label))
                        .collect::<Vec<_>>(),
                );
                let which = if hits.len() == 1 {
                    "use that lock instead of the site layer"
                } else {
                    "use those locks instead of the site layer"
                };
                family_notes.push(note(list, &entry, &name, format!("{described}; {which}")));
            }
        }
        if let Some(display) = ctx.licensed.get(&key) {
            licensed_notes.push(note(
                list,
                &entry,
                display,
                "which is group-restricted; keep it off the shared site layer".into(),
            ));
        }
    }
}

fn note(list: &str, entry: &Entry, name: &str, detail: String) -> LineNote {
    LineNote {
        list: list.to_string(),
        line: entry.line,
        file: entry.file.clone(),
        name: name.to_string(),
        detail,
    }
}

fn toolchain_detail(file: &str, families: &[FamilyLock], allowed: &[String]) -> Option<String> {
    let parsed = filename_toolchain(file);
    let ok = parsed
        .as_ref()
        .is_some_and(|split| allowed.iter().any(|label| label == &split.toolchain));
    if ok {
        return None;
    }
    let using = match &parsed {
        Some(split) => format!("uses={}", split.toolchain),
        None => "uses=none".to_string(),
    };
    let mut detail = format!(
        "{using}; a site-layer line on EESSI 2026.06 must use a 2026.1 toolchain or GCCcore-15.2.0 below it ({})",
        allowed.join(", ")
    );
    if let Some(split) = &parsed {
        let toolchain_name = split
            .toolchain
            .rsplit_once('-')
            .map(|(name, _)| name)
            .unwrap_or(split.toolchain.as_str());
        detail.push_str(&llvm_suffix(toolchain_name, families));
    }
    Some(detail)
}

fn llvm_suffix(toolchain_name: &str, families: &[FamilyLock]) -> String {
    if !LLVM_FAMILY
        .iter()
        .any(|family| family.eq_ignore_ascii_case(toolchain_name))
    {
        return String::new();
    }
    let locks: Vec<&FamilyLock> = families
        .iter()
        .filter(|family| {
            LLVM_FAMILY
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&family.toolchain_name))
        })
        .collect();
    if locks.is_empty() {
        return format!(" {toolchain_name} is the LLVM family, not a site-layer toolchain");
    }
    let described = english_join(
        &locks
            .iter()
            .map(|family| format!("{} ({})", family.path, family.label))
            .collect::<Vec<_>>(),
    );
    let noun = if locks.len() == 1 { "lock" } else { "locks" };
    format!(
        ". {toolchain_name} is the LLVM family, not a site-layer toolchain; the {noun} for that family is {described}"
    )
}

fn hook_flags(flags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        let flag = &flags[index];
        let key = flag.split('=').next().unwrap_or(flag);
        if key == "--hooks" || key == "--hook" {
            if flag.contains('=') || index + 1 == flags.len() || flags[index + 1].starts_with('-') {
                out.push(flag.clone());
            } else {
                out.push(format!("{flag} {}", flags[index + 1]));
                index += 1;
            }
        }
        index += 1;
    }
    out
}

fn english_join(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        many => {
            let (last, rest) = many.split_last().expect("at least three");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

fn render(report: &ProvidedReport, format: ProvidedFormat) -> Result<String> {
    let text = match format {
        ProvidedFormat::Json => {
            serde_json::to_string_pretty(report).context("provided report as JSON")? + "\n"
        }
        ProvidedFormat::Github => render_github(report),
        ProvidedFormat::Text => render_text(report),
    };
    Ok(text)
}

fn render_text(report: &ProvidedReport) -> String {
    let mut out = String::new();
    for hit in &report.hits {
        out.push_str(&format!(
            "provided {} name={} list={}\n",
            hit.file, hit.name, hit.list
        ));
    }
    for line in &report.unparsed {
        out.push_str(&format!("unparsed {} list={}\n", line.file, line.list));
    }
    for note in report
        .toolchain
        .iter()
        .map(|note| ("toolchain", note))
        .chain(report.hooks.iter().map(|note| ("hooks", note)))
        .chain(report.family.iter().map(|note| ("family", note)))
        .chain(report.licensed.iter().map(|note| ("licensed", note)))
    {
        out.push_str(&format!(
            "{} {} name={} list={} line={} {}\n",
            note.0, note.1.file, note.1.name, note.1.list, note.1.line, note.1.detail
        ));
    }
    out.push_str(&count_line(report));
    out
}

fn render_github(report: &ProvidedReport) -> String {
    let mut out = String::new();
    for hit in &report.hits {
        out.push_str(&github(
            "error",
            &hit.list,
            hit.line,
            "provided",
            &format!(
                "{} names {}, which the baseline already provides (name match; version ignored)",
                hit.file, hit.name
            ),
        ));
    }
    for line in &report.unparsed {
        out.push_str(&github(
            "error",
            &line.list,
            line.line,
            "unparsed",
            &format!("{} names no package", line.file),
        ));
    }
    for (kind, note) in report
        .toolchain
        .iter()
        .map(|note| ("toolchain", note))
        .chain(report.hooks.iter().map(|note| ("hooks", note)))
        .chain(report.family.iter().map(|note| ("family", note)))
    {
        out.push_str(&github(
            "error",
            &note.list,
            note.line,
            kind,
            &format!("{} name={} {}", note.file, note.name, note.detail),
        ));
    }
    for note in &report.licensed {
        out.push_str(&github(
            "warning",
            &note.list,
            note.line,
            "licensed",
            &format!("{} name={} {}", note.file, note.name, note.detail),
        ));
    }
    out.push_str(&count_line(report));
    out
}

fn count_line(report: &ProvidedReport) -> String {
    format!(
        "hits={} unparsed={} allowed={} provided={} toolchain={} hooks={} family={} licensed={}\n",
        report.hits.len(),
        report.unparsed.len(),
        report.allowed,
        report.provided,
        report.toolchain.len(),
        report.hooks.len(),
        report.family.len(),
        report.licensed.len(),
    )
}

fn github(severity: &str, file: &str, line: usize, title: &str, message: &str) -> String {
    format!(
        "::{severity} file={},line={line},title={title}::{}\n",
        github_property(file),
        github_message(message)
    )
}

fn github_property(text: &str) -> String {
    github_escape(text, true)
}

fn github_message(text: &str) -> String {
    github_escape(text, false)
}

fn github_escape(text: &str, property: bool) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '%' => out.push_str("%25"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            ':' if property => out.push_str("%3A"),
            ',' if property => out.push_str("%2C"),
            _ => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(text: &str, provided: &[&str], allow: &[&str]) -> ProvidedReport {
        let provided: BTreeSet<String> = provided.iter().map(|name| name.to_lowercase()).collect();
        let allow: BTreeSet<String> = allow.iter().map(|name| name.to_lowercase()).collect();
        let mut hits = Vec::new();
        let mut unparsed = Vec::new();
        let mut allowed_count = 0;
        classify_list(
            "site.list",
            text,
            &provided,
            &allow,
            &mut hits,
            &mut unparsed,
            &mut allowed_count,
        );
        ProvidedReport {
            hits,
            unparsed,
            allowed: allowed_count,
            provided: provided.len(),
            toolchain: Vec::new(),
            hooks: Vec::new(),
            family: Vec::new(),
            licensed: Vec::new(),
            strict: false,
            stdout: String::new(),
        }
    }

    #[test]
    fn a_name_the_baseline_has_fails_and_a_second_version_can_be_allowed() {
        let text = "\
# kept off the list
# FFTW-3.3.10-gompi-2026.1.eb
GROMACS-2026.0-foss-2026.1.eb
FFTW-3.3.10-gompi-2026.1.eb --hooks=/sw/hooks/x.py
fixtures/foss-2025b/Python-3.12.3-foss-2025b.eb
Vendor-1.2.eb
not-a-version.eb
";
        let report = classify(text, &["GROMACS", "fftw", "Python"], &["gromacs"]);
        assert_eq!(report.hits.len(), 2, "{:?}", report.hits);
        assert_eq!(report.hits[0].name, "FFTW");
        assert_eq!(report.hits[1].name, "Python");
        assert_eq!(
            report.hits[1].file,
            "fixtures/foss-2025b/Python-3.12.3-foss-2025b.eb"
        );
        assert_eq!(report.allowed, 1);
        assert_eq!(report.unparsed.len(), 1);
        assert_eq!(report.unparsed[0].file, "not-a-version.eb");
        assert!(report
            .failure()
            .unwrap()
            .contains("2 build-list lines name"));
        assert!(report
            .failure()
            .unwrap()
            .contains("1 build-list line names no package"));
    }

    #[test]
    fn a_list_of_site_only_names_passes() {
        let report = classify("VASP-6.5.1.eb\n", &["GROMACS"], &[]);
        assert!(report.hits.is_empty());
        assert!(report.unparsed.is_empty());
        assert!(report.failure().is_none());
    }
}
