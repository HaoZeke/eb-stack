//! Decide a bumped patch set from the new source itself.
//!
//! Without a same-version sibling in the tree, [`crate::patch_evolution`]
//! can only flag a patch whose name pins another release. When the bump has
//! downloaded the new source, the strongest evidence is the one EasyBuild
//! will produce at build time: whether `patch` accepts the file against the
//! unpacked source. Each patch is tried in recipe order, the way
//! `EasyBlock.patch_step` applies them, with EasyBuild's own command
//! (`patch -p<level> -i <file>`, plus the entry's `opts`) and the level it
//! would guess from the files the patch names:
//!
//! * the patch applies: it is carried, and applied to the scratch tree so a
//!   later patch sees what it would see at build time;
//! * the patch reverse-applies: the new release already contains it, so it is
//!   dropped from `patches` and from `checksums`;
//! * none of the files it modifies exists in the new source: the code it
//!   fixed is gone, so it is dropped;
//! * anything else stays undecided, with the hunks that failed as evidence.
//!
//! Nothing here writes a patch. A patch that needs rebasing is a residual.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::eb_parse::resolve_easyconfig_str;
use crate::package::ResidualSeverity;
use crate::package_workflow::PackageBundle;
use crate::source_fetch::SourceFetch;

/// A directory under the system temporary directory, removed on drop, that
/// holds the unpacked source while the patches are tried.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "eb-stack-patch-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the new source says about one patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceVerdict {
    /// `patch` accepts it at this level.
    Applies(u32),
    /// `patch -R` accepts it at this level: the release already has it.
    Upstream(u32),
    /// None of the files it modifies exists in the source.
    TargetsGone(Vec<String>),
    /// Neither direction applies; the lines `patch` reported.
    Fails(String),
    /// The entry only copies a file into the build directory.
    CopyOnly,
}

/// One `patches` entry as EasyBuild reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchEntry {
    /// Resolved patch file name.
    pub name: String,
    /// `level` from a `(name, N)` tuple or a dict's `'level'`.
    pub level: Option<u32>,
    /// A dict's `'opts'`, passed to `patch` after the file.
    pub opts: Option<String>,
    /// A `(name, 'subdir')` tuple or a dict with `'copy'`: not applied.
    pub copy_only: bool,
}

/// Read the `patches` entries of `text` in order, with the resolved names.
pub(crate) fn patch_entries(text: &str) -> Vec<PatchEntry> {
    let Ok(recipe) = resolve_easyconfig_str(text) else {
        return Vec::new();
    };
    let Ok(Some((start, end))) = crate::eb_emit::easyconfig_list_span(text, "patches") else {
        return Vec::new();
    };
    let body = &text[start..end];
    let spans = entry_spans(body);
    if spans.len() != recipe.patch_names.len() {
        return Vec::new();
    }
    spans
        .iter()
        .zip(&recipe.patch_names)
        .map(|((s, e), name)| parse_entry(strip_comments(&body[*s..*e]).trim(), name))
        .collect()
}

/// Entry spans of a list body without the comment lines that open them: the
/// comment trailing one entry lands at the head of the next span when the
/// list is split on commas, and belongs to the line it is on.
fn entry_spans(body: &str) -> Vec<(usize, usize)> {
    crate::index_bump::top_level_spans(body)
        .into_iter()
        .filter_map(|(start, end)| {
            let skip = crate::index_bump::leading_comment_len(&body[start..end]);
            (start + skip < end).then_some((start + skip, end))
        })
        .collect()
}

fn strip_comments(entry: &str) -> String {
    entry
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_entry(raw: &str, name: &str) -> PatchEntry {
    let mut entry = PatchEntry {
        name: name.to_string(),
        level: None,
        opts: None,
        copy_only: false,
    };
    if raw.starts_with('(') {
        let inner = raw.trim_start_matches('(').trim_end_matches(')');
        if let Some((_, second)) = inner.split_once(',') {
            let second = second.trim().trim_end_matches(',').trim();
            if let Ok(level) = second.parse::<u32>() {
                entry.level = Some(level);
            } else if second.starts_with('\'') || second.starts_with('"') {
                entry.copy_only = true;
            }
        }
    } else if raw.starts_with('{') {
        if let Some(level) = dict_value(raw, "level").and_then(|v| v.parse::<u32>().ok()) {
            entry.level = Some(level);
        }
        entry.opts = dict_value(raw, "opts").map(|v| unquote(&v));
        entry.copy_only = dict_value(raw, "copy").is_some();
    }
    entry
}

fn dict_value(raw: &str, key: &str) -> Option<String> {
    let pattern =
        regex::Regex::new(&format!(r#"['"]{key}['"]\s*:\s*('[^']*'|"[^"]*"|[^,}}]+)"#)).ok()?;
    pattern
        .captures(raw)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn unquote(value: &str) -> String {
    value.trim_matches(|c| c == '\'' || c == '"').to_string()
}

/// Files a patch creates or modifies, from its `+++` lines, as EasyBuild's
/// `det_patched_files` reads them. The flag is true when the matching `---`
/// side is `/dev/null`, so the patch creates the file.
pub(crate) fn patched_files(patch: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut previous_minus: Option<String> = None;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            previous_minus = rest.split_whitespace().next().map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(path) = rest.split_whitespace().next() {
                if path == "/dev/null" {
                    continue;
                }
                let created = previous_minus.as_deref() == Some("/dev/null");
                if !out.iter().any(|(seen, _): &(String, bool)| seen == path) {
                    out.push((path.to_string(), created));
                }
            }
            previous_minus = None;
        }
    }
    out
}

/// EasyBuild's `guess_patch_level`: the first patched file that exists in
/// `root` once its leading components are stripped decides the level.
pub(crate) fn guess_patch_level(files: &[(String, bool)], root: &Path) -> Option<u32> {
    for (file, _) in files {
        let parts: Vec<&str> = file.split('/').filter(|part| !part.is_empty()).collect();
        for level in 0..parts.len() {
            if root.join(parts[level..].join("/")).is_file() {
                return Some(level as u32);
            }
        }
    }
    None
}

fn run_patch(
    root: &Path,
    patch: &Path,
    level: u32,
    extra: &[&str],
    opts: Option<&str>,
) -> (bool, String) {
    let mut command = Command::new("patch");
    command
        .current_dir(root)
        .args(extra)
        .arg(format!("-p{level}"))
        .arg("-i")
        .arg(patch);
    if let Some(opts) = opts {
        command.args(opts.split_whitespace());
    }
    match command.output() {
        Ok(output) => (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => (false, format!("patch could not run: {error}")),
    }
}

/// The lines of `patch` output that say why it refused.
fn failure_lines(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| {
            line.contains("FAILED")
                || line.contains("No file to patch")
                || line.contains("can't find file")
                || line.contains("Reversed (or previously applied)")
                || line.contains("could not run")
                || line.contains("would create the file")
                || line.contains("which already exists")
                || line.starts_with("checking file")
        })
        .collect();
    let mut kept: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        // A `checking file` line is kept only when the next line is its failure.
        if line.starts_with("checking file") {
            if lines
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("checking file"))
            {
                kept.push(line);
            }
        } else {
            kept.push(line);
        }
    }
    kept.truncate(6);
    kept.join("; ")
}

/// Judge one patch against the scratch tree, applying it when it applies.
pub(crate) fn judge(entry: &PatchEntry, patch_path: &Path, root: &Path) -> SourceVerdict {
    if entry.copy_only {
        return SourceVerdict::CopyOnly;
    }
    let text = match std::fs::read(patch_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => return SourceVerdict::Fails(format!("patch file unreadable: {error}")),
    };
    let files = patched_files(&text);
    let level = entry.level.or_else(|| guess_patch_level(&files, root));
    let Some(level) = level else {
        let modified: Vec<String> = files
            .iter()
            .filter(|(_, created)| !created)
            .map(|(file, _)| file.clone())
            .collect();
        if !modified.is_empty() && modified.len() == files.len() {
            return SourceVerdict::TargetsGone(modified);
        }
        return SourceVerdict::Fails("no file the patch names exists at any strip level".into());
    };
    let opts = entry.opts.as_deref();
    let (forward, forward_output) = run_patch(
        root,
        patch_path,
        level,
        &["--dry-run", "-N", "-f", "-s"],
        opts,
    );
    if forward {
        let (applied, output) = run_patch(root, patch_path, level, &["-N", "-f", "-s"], opts);
        if !applied {
            return SourceVerdict::Fails(failure_lines(&output));
        }
        return SourceVerdict::Applies(level);
    }
    let (reverse, _) = run_patch(
        root,
        patch_path,
        level,
        &["--dry-run", "-R", "-f", "-s"],
        opts,
    );
    if reverse {
        return SourceVerdict::Upstream(level);
    }
    let (_, verbose) = run_patch(root, patch_path, level, &["--dry-run", "-N", "-f"], opts);
    let reason = failure_lines(&verbose);
    let reason = if reason.is_empty() {
        failure_lines(&forward_output)
    } else {
        reason
    };
    SourceVerdict::Fails(if reason.is_empty() {
        format!("patch -p{level} exited non-zero without naming a hunk")
    } else {
        reason
    })
}

/// Unpack `bytes` saved as `filename` under `into`, and return the
/// directory EasyBuild would patch: the single top-level directory when the
/// archive has one, else `into` itself.
pub(crate) fn unpack(filename: &str, bytes: &[u8], into: &Path) -> Result<PathBuf, String> {
    let archive = into.join(".archive").join(filename);
    std::fs::create_dir_all(archive.parent().unwrap_or(into))
        .map_err(|error| format!("scratch directory: {error}"))?;
    std::fs::write(&archive, bytes).map_err(|error| format!("write archive: {error}"))?;
    let tree = into.join("tree");
    std::fs::create_dir_all(&tree).map_err(|error| format!("scratch tree: {error}"))?;
    let lower = filename.to_ascii_lowercase();
    let mut command = if lower.ends_with(".zip") {
        let mut command = Command::new("unzip");
        command.arg("-q").arg(&archive).arg("-d").arg(&tree);
        command
    } else {
        let mut command = Command::new("tar");
        command.arg("-xf").arg(&archive).arg("-C").arg(&tree);
        command
    };
    let output = command
        .output()
        .map_err(|error| format!("unpack {filename}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "unpack {filename}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(&tree)
        .map_err(|error| format!("read unpacked tree: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    if entries.len() == 1 && entries[0].is_dir() {
        return Ok(entries[0].clone());
    }
    Ok(tree)
}

/// Summary prefix [`crate::package_workflow`] writes for a patch decided
/// without a sibling, which is the set this module may revisit.
fn residual_prefixes(name: &str) -> [String; 2] {
    [
        format!("undecided {name}: no "),
        format!("carry {name}: no "),
    ]
}

/// Revisit every patch the tree could not decide against the downloaded
/// source, rewrite the recipe for the ones the source drops, and replace
/// their decision records. Returns the names still undecided.
pub(crate) fn decide_from_source(
    bundle: &mut PackageBundle,
    new_version: &str,
    fetched: &[SourceFetch],
) -> Vec<String> {
    let Some(easyconfig) = bundle.easyconfigs.first() else {
        return Vec::new();
    };
    let entries = patch_entries(&easyconfig.text);
    let open: Vec<&PatchEntry> = entries
        .iter()
        .filter(|entry| {
            let prefixes = residual_prefixes(&entry.name);
            bundle.plan.residuals.iter().any(|residual| {
                residual.category == "patch-decision"
                    && prefixes.iter().any(|prefix| {
                        residual.summary.starts_with(prefix)
                            && residual.summary[prefix.len()..]
                                .starts_with(&format!("{new_version} sibling"))
                    })
            })
        })
        .collect();
    if open.is_empty() {
        return undecided_names(bundle);
    }
    let Some(source) = fetched.first() else {
        return undecided_names(bundle);
    };
    let Some(bytes) = source.bytes.as_deref() else {
        return undecided_names(bundle);
    };
    let Ok(scratch) = Scratch::new() else {
        return undecided_names(bundle);
    };
    let root = match unpack(&source.filename, bytes, scratch.path()) {
        Ok(root) => root,
        Err(_) => return undecided_names(bundle),
    };
    let label = format!("{} source", source.filename);
    let mut dropped = Vec::new();
    let mut judged = 0usize;
    for entry in &entries {
        let is_open = open.iter().any(|candidate| candidate.name == entry.name);
        let path = bundle
            .plan
            .build
            .patches
            .iter()
            .find(|patch| patch.filename == entry.name)
            .and_then(|patch| {
                patch
                    .resolved_source
                    .clone()
                    .or_else(|| patch.source.as_ref().map(PathBuf::from))
            });
        let Some(path) = path else {
            continue;
        };
        let verdict = judge(entry, &path, &root);
        if !is_open {
            continue;
        }
        judged += 1;
        let (decision, evidence, severity) = verdict_record(&verdict, &label);
        if decision == "drop" {
            dropped.push(entry.name.clone());
        }
        let prefixes = residual_prefixes(&entry.name);
        if let Some(residual) = bundle.plan.residuals.iter_mut().find(|residual| {
            residual.category == "patch-decision"
                && prefixes
                    .iter()
                    .any(|prefix| residual.summary.starts_with(prefix))
        }) {
            residual.summary = format!("{decision} {}: {evidence}", entry.name);
            residual.severity = severity;
        }
    }
    if !dropped.is_empty() {
        if let Some(easyconfig) = bundle.easyconfigs.first_mut() {
            easyconfig.text = remove_patches(&easyconfig.text, &dropped);
        }
        bundle
            .plan
            .build
            .patches
            .retain(|patch| !dropped.contains(&patch.filename));
    }
    // Per-patch source evidence supersedes the blanket review warning, as a
    // sibling's patch list does; a patch the source leaves undecided keeps
    // its own judgment record.
    if judged == open.len() {
        bundle.plan.residuals.retain(|residual| {
            !(residual.category == "bump-warning"
                && residual.summary.contains("patches were not modified"))
        });
    }
    undecided_names(bundle)
}

/// Judge the patches of the extension that is the package itself against
/// the extension archive at the new version, as [`decide_from_source`] does
/// for the recipe's own `patches`. EasyBuild applies them in the unpacked
/// extension, so a bump that moves the extension moves the evidence too.
pub(crate) fn decide_extension_patches(
    bundle: &mut PackageBundle,
    prior_version: Option<&str>,
    client: &dyn crate::registry::RegistryClient,
) {
    let Some(easyconfig) = bundle.easyconfigs.first() else {
        return;
    };
    let text = easyconfig.text.clone();
    let Ok(recipe) = resolve_easyconfig_str(&text) else {
        return;
    };
    if prior_version.is_none_or(|prior| prior == recipe.version) {
        return;
    }
    let Some(own) = crate::index_bump::own_extension(&text, client) else {
        return;
    };
    let raw = &text[own.span.0..own.span.1];
    let Some((patch_start, patch_end)) = crate::index_bump::dict_list_span(raw, "patches") else {
        return;
    };
    let body = &raw[patch_start..patch_end];
    let entries: Vec<(PatchEntry, (usize, usize))> = entry_spans(body)
        .into_iter()
        .filter_map(|(s, e)| {
            let entry = strip_comments(&body[s..e]);
            let name = first_literal(&entry)?;
            Some((parse_entry(entry.trim(), &name), (s, e)))
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    let Ok(scratch) = Scratch::new() else {
        return;
    };
    let label = format!("{} {} extension archive", recipe.name, recipe.version);
    let root = match unpack("extension.tar.gz", &own.archive, scratch.path()) {
        Ok(root) => root,
        Err(_) => return,
    };
    let mut dropped: Vec<String> = Vec::new();
    for (index, (entry, _)) in entries.iter().enumerate() {
        let path = bundle
            .plan
            .build
            .patches
            .iter()
            .find(|patch| patch.filename == entry.name)
            .and_then(|patch| {
                patch
                    .resolved_source
                    .clone()
                    .or_else(|| patch.source.as_ref().map(PathBuf::from))
            });
        let (decision, evidence, severity) = match path {
            None => (
                "undecided",
                "the patch file is not beside the source recipe".to_string(),
                ResidualSeverity::Judgment,
            ),
            Some(path) => verdict_record(&judge(entry, &path, &root), &label),
        };
        if decision == "drop" {
            dropped.push(entry.name.clone());
        }
        bundle.plan.residuals.push(crate::package::Residual {
            id: format!("patch-decision:extension:{index}"),
            stage: crate::package::ResidualStage::Emit,
            category: "patch-decision".into(),
            severity,
            summary: format!("{decision} {}: {evidence}", entry.name),
            evidence: None,
            provenance: None,
        });
    }
    if dropped.is_empty() {
        return;
    }
    let mut rewritten = raw.to_string();
    for key in ["checksums", "patches"] {
        let Some((start, end)) = crate::index_bump::dict_list_span(&rewritten, key) else {
            continue;
        };
        let list = rewritten[start..end].to_string();
        let remove: Vec<(usize, usize)> = entry_spans(&list)
            .into_iter()
            .filter(|(s, e)| {
                let own_text = &list[*s..*e];
                let trailing = list[*e..].lines().next().unwrap_or("");
                dropped.iter().any(|name| {
                    own_text.contains(name.as_str()) || trailing.contains(name.as_str())
                })
            })
            .collect();
        let cut = remove_spans(&list, &remove);
        rewritten = format!("{}{}{}", &rewritten[..start], cut, &rewritten[end..]);
    }
    if let Ok(empty) = regex::Regex::new(r#"\n[ \t]*['"]patches['"]\s*:\s*\[\s*\],?"#) {
        rewritten = empty.replace(&rewritten, "").into_owned();
    }
    if let Some(easyconfig) = bundle.easyconfigs.first_mut() {
        easyconfig.text = format!(
            "{}{}{}",
            &text[..own.span.0],
            rewritten,
            &text[own.span.1..]
        );
    }
    bundle
        .plan
        .build
        .patches
        .retain(|patch| !dropped.contains(&patch.filename));
}

/// First quoted literal of an entry: the patch name of a string, tuple or
/// `{'name': ...}` dict entry.
fn first_literal(entry: &str) -> Option<String> {
    let trimmed = entry.trim();
    if trimmed.starts_with('{') {
        return dict_value(trimmed, "name").map(|value| unquote(&value));
    }
    let pattern = regex::Regex::new(r#"'([^'\n]*)'|"([^"\n]*)""#).ok()?;
    let caps = pattern.captures(trimmed)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .map(|m| m.as_str().to_string())
}

/// Decision, evidence and severity for one source verdict.
fn verdict_record(
    verdict: &SourceVerdict,
    label: &str,
) -> (&'static str, String, ResidualSeverity) {
    match verdict {
        SourceVerdict::Applies(level) => (
            "carry",
            format!("applies to the {label} at -p{level}"),
            ResidualSeverity::Mechanical,
        ),
        SourceVerdict::Upstream(level) => (
            "drop",
            format!("the {label} already contains it: it reverse-applies at -p{level}"),
            ResidualSeverity::Mechanical,
        ),
        SourceVerdict::TargetsGone(files) => (
            "drop",
            format!(
                "no file it modifies exists in the {label} ({})",
                files.join(", ")
            ),
            ResidualSeverity::Mechanical,
        ),
        SourceVerdict::CopyOnly => (
            "carry",
            "copied into the build directory, not applied".to_string(),
            ResidualSeverity::Mechanical,
        ),
        SourceVerdict::Fails(lines) => (
            "undecided",
            format!("does not apply to the {label} and does not reverse-apply: {lines}"),
            ResidualSeverity::Judgment,
        ),
    }
}

fn undecided_names(bundle: &PackageBundle) -> Vec<String> {
    bundle
        .plan
        .residuals
        .iter()
        .filter(|residual| residual.category == "patch-decision")
        .filter_map(|residual| residual.summary.strip_prefix("undecided "))
        .filter_map(|rest| rest.split_once(": ").map(|(name, _)| name.to_string()))
        .collect()
}

/// Remove the named patches from `patches` and their entries from
/// `checksums`. A checksum entry is found by its key or trailing comment,
/// else by position when the list is purely positional.
pub(crate) fn remove_patches(text: &str, names: &[String]) -> String {
    let Ok(recipe) = resolve_easyconfig_str(text) else {
        return text.to_string();
    };
    let positions: Vec<usize> = recipe
        .patch_names
        .iter()
        .enumerate()
        .filter(|(_, name)| names.contains(name))
        .map(|(index, _)| index)
        .collect();
    let mut out = text.to_string();
    // Checksums first: positional lookup needs the patch indices of the
    // recipe as written.
    if let Ok(Some((start, end))) = crate::eb_emit::easyconfig_list_span(&out, "checksums") {
        let body = out[start..end].to_string();
        let spans = entry_spans(&body);
        let positional = recipe.checksum_entry_keys.iter().all(Vec::is_empty)
            && spans.len() == recipe.sources_count + recipe.patch_names.len();
        let mut remove: Vec<(usize, usize)> = Vec::new();
        for (patch_index, name) in recipe.patch_names.iter().enumerate() {
            if !names.contains(name) {
                continue;
            }
            let by_name = spans.iter().position(|(s, e)| {
                // The span holds the entry and, for the last entry, its inline
                // comment; a comment after a comma follows the span.
                let own = &body[*s..*e];
                let trailing = body[*e..].lines().next().unwrap_or("");
                literal_names(own, name, &recipe)
                    || own.contains(name.as_str())
                    || trailing.contains(name.as_str())
            });
            let found =
                by_name.or_else(|| positional.then_some(recipe.sources_count + patch_index));
            if let Some(index) = found {
                if let Some(span) = spans.get(index) {
                    remove.push(*span);
                }
            }
        }
        let rewritten = remove_spans(&body, &remove);
        out = format!("{}{}{}", &out[..start], rewritten, &out[end..]);
    }
    if let Ok(Some((start, end))) = crate::eb_emit::easyconfig_list_span(&out, "patches") {
        let body = out[start..end].to_string();
        let spans = entry_spans(&body);
        let remove: Vec<(usize, usize)> = positions
            .iter()
            .filter_map(|index| spans.get(*index).copied())
            .collect();
        let rewritten = remove_spans(&body, &remove);
        out = format!("{}{}{}", &out[..start], rewritten, &out[end..]);
        if entry_spans(&rewritten).is_empty() {
            if let Ok(Some((assign_start, assign_end))) =
                crate::eb_emit::find_list_assignment_span(&out, "patches")
            {
                let mut cut_end = assign_end;
                if out[cut_end..].starts_with('\n') {
                    cut_end += 1;
                }
                out = format!("{}{}", &out[..assign_start], &out[cut_end..]);
            }
        }
    }
    out
}

/// Whether a checksum entry's key spells `name`, literally or through the
/// name and version templates.
fn literal_names(entry: &str, name: &str, recipe: &crate::eb_parse::ResolvedEasyconfig) -> bool {
    let Ok(literal) = regex::Regex::new(r#"'([^'\n]*)'|"([^"\n]*)""#) else {
        return false;
    };
    let named = literal.captures_iter(entry).any(|caps| {
        let inner = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        let expanded = inner
            .replace("%(namelower)s", &recipe.name.to_ascii_lowercase())
            .replace("%(name)s", &recipe.name)
            .replace("%(version)s", &recipe.version);
        expanded == name
    });
    named
}

/// Cut list entries out of a list body. An entry alone on its lines goes
/// with its lines, trailing comma and comment; an inline one goes with the
/// comma and space that separate it from its neighbour.
fn remove_spans(body: &str, spans: &[(usize, usize)]) -> String {
    let mut spans = spans.to_vec();
    spans.sort_unstable();
    spans.dedup();
    let mut out = body.to_string();
    for (start, end) in spans.into_iter().rev() {
        let line_start = out[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let alone_before = out[line_start..start].trim().is_empty() && line_start > 0;
        let after = &out[end..];
        let line_end = after.find('\n').map(|i| end + i);
        let tail = line_end.map(|le| &out[end..le]).unwrap_or(after);
        let tail_trim = tail.trim_start().trim_start_matches(',').trim_start();
        let alone_after = tail_trim.is_empty() || tail_trim.starts_with('#');
        if alone_before && alone_after {
            if let Some(le) = line_end {
                out.replace_range(line_start..le + 1, "");
                continue;
            }
        }
        // Inline: take the following comma and whitespace, else the
        // preceding ones.
        let mut cut_end = end;
        let rest = &out[end..];
        let skipped = rest.len() - rest.trim_start().len();
        if rest.trim_start().starts_with(',') {
            cut_end = end + skipped + 1;
            let more = &out[cut_end..];
            cut_end += more.len() - more.trim_start().len();
            out.replace_range(start..cut_end, "");
        } else {
            let before = &out[..start];
            let trimmed = before.trim_end();
            let cut_start = if trimmed.ends_with(',') {
                trimmed.len() - 1
            } else {
                start
            };
            out.replace_range(cut_start..cut_end, "");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_levels_opts_and_copies() {
        let text = "name = 'Pkg'\nversion = '2.0'\ntoolchain = SYSTEM\n\
            patches = [\n    'a.patch',\n    ('b.patch', 1),\n    ('c.txt', 'sub'),\n    \
            {'name': 'd.patch', 'opts': '--binary', 'level': 2},\n]\n";
        let entries = patch_entries(text);
        assert_eq!(entries.len(), 4, "{entries:?}");
        assert_eq!(entries[0].level, None);
        assert_eq!(entries[1].level, Some(1));
        assert!(entries[2].copy_only);
        assert_eq!(entries[3].level, Some(2));
        assert_eq!(entries[3].opts.as_deref(), Some("--binary"));
    }

    #[test]
    fn patched_files_mark_created_files() {
        let patch = "--- a/src/x.c\n+++ b/src/x.c\n@@\n--- /dev/null\n+++ b/src/new.c\n@@\n";
        assert_eq!(
            patched_files(patch),
            vec![
                ("b/src/x.c".to_string(), false),
                ("b/src/new.c".to_string(), true)
            ]
        );
    }

    #[test]
    fn the_level_is_guessed_from_the_first_file_that_exists() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/x.c"), "x").unwrap();
        let files = vec![("pkg-1.0.orig/src/x.c".to_string(), false)];
        assert_eq!(guess_patch_level(&files, temp.path()), Some(1));
        let gone = vec![("a/plugins/gone.c".to_string(), false)];
        assert_eq!(guess_patch_level(&gone, temp.path()), None);
    }

    #[test]
    fn a_dropped_patch_leaves_patches_and_its_checksum() {
        let text = "name = 'Pkg'\nversion = '2.0'\ntoolchain = SYSTEM\n\
            sources = ['pkg-2.0.tar.gz']\n\
            patches = [\n    'Pkg-1.0_a.patch',\n    'Pkg-1.0_b.patch',\n]\n\
            checksums = [\n    {'pkg-2.0.tar.gz': 'aa'},\n    {'Pkg-1.0_a.patch': 'bb'},\n    \
            {'Pkg-1.0_b.patch':\n     'cc'},\n]\nmoduleclass = 'tools'\n";
        let out = remove_patches(text, &["Pkg-1.0_b.patch".to_string()]);
        assert!(
            out.contains("patches = [\n    'Pkg-1.0_a.patch',\n]"),
            "{out}"
        );
        assert!(!out.contains("'cc'"), "{out}");
        assert!(out.contains("{'Pkg-1.0_a.patch': 'bb'},\n]"), "{out}");
    }

    #[test]
    fn dropping_every_patch_removes_the_list_and_positional_checksums() {
        let text = "name = 'Pkg'\nversion = '2.0'\ntoolchain = SYSTEM\n\
            sources = ['pkg-2.0.tar.gz']\n\
            patches = ['Pkg_one.patch',\n           'Pkg_two.patch']\n\
            checksums = [\n    'aa',  # pkg-2.0.tar.gz\n    'bb',  # Pkg_one.patch\n    'cc',  # Pkg_two.patch\n]\n\
            moduleclass = 'tools'\n";
        let out = remove_patches(
            text,
            &["Pkg_one.patch".to_string(), "Pkg_two.patch".to_string()],
        );
        assert!(!out.contains("patches"), "{out}");
        assert!(
            out.contains("checksums = [\n    'aa',  # pkg-2.0.tar.gz\n]"),
            "{out}"
        );
    }

    #[test]
    fn a_patch_is_carried_dropped_or_left_by_what_patch_says() {
        if Command::new("patch").arg("--version").output().is_err() {
            panic!("GNU patch is required by this test");
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("pkg-2.0");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let entry = PatchEntry {
            name: "fix.patch".into(),
            level: None,
            opts: None,
            copy_only: false,
        };
        let applies = temp.path().join("applies.patch");
        std::fs::write(
            &applies,
            "--- pkg.orig/a.txt\n+++ pkg/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
        )
        .unwrap();
        assert_eq!(judge(&entry, &applies, &root), SourceVerdict::Applies(1));
        // Applied to the scratch tree, so the same patch now reverse-applies.
        assert_eq!(judge(&entry, &applies, &root), SourceVerdict::Upstream(1));
        let fails = temp.path().join("fails.patch");
        std::fs::write(
            &fails,
            "--- pkg.orig/a.txt\n+++ pkg/a.txt\n@@ -1,3 +1,3 @@\n uno\n-dos\n+DOS\n tres\n",
        )
        .unwrap();
        assert!(matches!(
            judge(&entry, &fails, &root),
            SourceVerdict::Fails(_)
        ));
        let gone = temp.path().join("gone.patch");
        std::fs::write(
            &gone,
            "--- pkg.orig/plugins/x.c\n+++ pkg/plugins/x.c\n@@ -1 +1 @@\n-a\n+b\n",
        )
        .unwrap();
        assert_eq!(
            judge(&entry, &gone, &root),
            SourceVerdict::TargetsGone(vec!["pkg/plugins/x.c".into()])
        );
    }
}
