//! Sandboxed reads and edits for recipe workspaces.
//!
//! `eb_recipe_read`, `eb_recipe_list`, `eb_recipe_grep`, `eb_recipe_edit`, and
//! `eb_recipe_write` resolve every path against one workspace root. A `..` component, an absolute
//! path outside that root, or a symlink whose target leaves the root is rejected
//! before the file is opened.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Largest recipe-workspace file the file tools will read or write.
pub const MAX_WORKSPACE_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// UTF-8 file contents from the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceText {
    /// Path relative to the workspace root when the request was inside it.
    pub path: String,
    /// File contents.
    pub content: String,
    /// Byte length of `content`.
    pub bytes: usize,
    /// Number of `\n`-separated lines, including a trailing empty line.
    pub lines: usize,
}

/// One directory entry. Symlinks are not followed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceEntry {
    /// File name inside the listed directory.
    pub name: String,
    /// `file`, `dir`, `symlink`, or `other`.
    pub kind: String,
    /// True when this entry is a symlink whose target leaves the workspace.
    pub escapes: bool,
}

/// Directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceList {
    /// Listed path, relative to the workspace root.
    pub path: String,
    /// Entries sorted by name.
    pub entries: Vec<WorkspaceEntry>,
}

/// Result of an exact string replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceEdit {
    /// Edited path, relative to the workspace root.
    pub path: String,
    /// Number of times `old` was replaced.
    pub replaced: usize,
    /// Byte length after the edit.
    pub bytes: usize,
}

/// Result of creating or overwriting a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceWrite {
    /// Written path, relative to the workspace root.
    pub path: String,
    /// True when the file did not exist before this call.
    pub created: bool,
    /// Byte length written.
    pub bytes: usize,
}

/// Default and upper bound for the number of lines [`grep_workspace`] returns.
pub const MAX_GREP_MATCHES: usize = 200;

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceMatch {
    /// File path relative to the workspace root.
    pub path: String,
    /// 1-based line number.
    pub line: usize,
    /// The line, without its newline.
    pub text: String,
}

/// Result of searching the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceGrep {
    /// The pattern searched for.
    pub pattern: String,
    /// True when `pattern` was a regular expression, false for literal text.
    pub regex: bool,
    /// Matching lines, in path then line order.
    pub matches: Vec<WorkspaceMatch>,
    /// Number of UTF-8 text files read.
    pub files_searched: usize,
    /// True when the search stopped at the match limit.
    pub truncated: bool,
}

/// Workspace root for the recipe file tools.
///
/// `EB_STACK_WORKSPACE` wins when it is set and non-empty. Otherwise the
/// server's current directory is the root. `eb-stack mcp --workspace` sets
/// the environment variable before serving.
pub fn configured_workspace_root() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("EB_STACK_WORKSPACE") {
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    std::env::current_dir().map_err(|error| format!("current directory: {error}"))
}

/// Read a UTF-8 file that resolves inside `root`.
pub fn read_workspace(root: &Path, requested: &Path) -> Result<WorkspaceText, String> {
    let (root, path) = resolve_inside(root, requested)?;
    let metadata =
        fs::metadata(&path).map_err(|error| format!("read {}: {error}", requested.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", requested.display()));
    }
    if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
        return Err(format!(
            "{} is larger than {MAX_WORKSPACE_FILE_BYTES} bytes",
            requested.display()
        ));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", requested.display()))?;
    Ok(WorkspaceText {
        path: relative_display(&root, &path, requested),
        lines: content.split('\n').count(),
        bytes: content.len(),
        content,
    })
}

/// List one directory inside `root` without following symlinks.
pub fn list_workspace(root: &Path, requested: &Path) -> Result<WorkspaceList, String> {
    let (root, path) = resolve_inside(root, requested)?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", requested.display()));
    }
    let mut entries = Vec::new();
    let directory =
        fs::read_dir(&path).map_err(|error| format!("list {}: {error}", requested.display()))?;
    for entry in directory {
        let entry = entry.map_err(|error| format!("list {}: {error}", requested.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("list {}: {error}", requested.display()))?;
        let (kind, escapes) = if file_type.is_symlink() {
            ("symlink", symlink_escapes(&root, &entry.path()))
        } else if file_type.is_dir() {
            ("dir", false)
        } else if file_type.is_file() {
            ("file", false)
        } else {
            ("other", false)
        };
        entries.push(WorkspaceEntry {
            name,
            kind: kind.to_string(),
            escapes,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(WorkspaceList {
        path: relative_display(&root, &path, requested),
        entries,
    })
}

/// Search UTF-8 files under `requested` (a file or directory inside `root`).
///
/// `pattern` is literal text unless `regex` is true. Symlinks are not
/// followed, `.git` is skipped, and files that are not UTF-8 or are larger
/// than [`MAX_WORKSPACE_FILE_BYTES`] are skipped. At most `max_matches`
/// lines are returned (default and cap [`MAX_GREP_MATCHES`]).
pub fn grep_workspace(
    root: &Path,
    requested: &Path,
    pattern: &str,
    regex: bool,
    max_matches: Option<usize>,
) -> Result<WorkspaceGrep, String> {
    if pattern.is_empty() {
        return Err("pattern is empty".into());
    }
    let matcher = if regex {
        Some(
            regex::Regex::new(pattern)
                .map_err(|error| format!("invalid regular expression {pattern}: {error}"))?,
        )
    } else {
        None
    };
    let (root, start) = resolve_inside(root, requested)?;
    let limit = max_matches
        .unwrap_or(MAX_GREP_MATCHES)
        .clamp(1, MAX_GREP_MATCHES);
    let mut result = WorkspaceGrep {
        pattern: pattern.to_string(),
        regex,
        matches: Vec::new(),
        files_searched: 0,
        truncated: false,
    };
    let mut pending = vec![start];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let Ok(directory) = fs::read_dir(&path) else {
                continue;
            };
            let mut children: Vec<PathBuf> = directory
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name() != ".git")
                .map(|entry| entry.path())
                .collect();
            children.sort();
            pending.extend(children.into_iter().rev());
            continue;
        }
        if !metadata.is_file() || metadata.len() > MAX_WORKSPACE_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        result.files_searched += 1;
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (index, line) in content.lines().enumerate() {
            let hit = match &matcher {
                Some(matcher) => matcher.is_match(line),
                None => line.contains(pattern),
            };
            if !hit {
                continue;
            }
            if result.matches.len() == limit {
                result.truncated = true;
                return Ok(result);
            }
            result.matches.push(WorkspaceMatch {
                path: shown.clone(),
                line: index + 1,
                text: line.to_string(),
            });
        }
    }
    Ok(result)
}

/// Replace an exact `old` string in a workspace file.
///
/// `old` is literal text. When it matches more than once and `replace_all` is
/// false, the file is left unchanged and the error tells the caller to pass
/// `replace_all` or a longer unique string.
pub fn edit_workspace(
    root: &Path,
    requested: &Path,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<WorkspaceEdit, String> {
    if old.is_empty() {
        return Err("old string must not be empty".into());
    }
    if old == new {
        return Err("old and new are the same string".into());
    }
    let (root, path) = resolve_inside(root, requested)?;
    if !path.is_file() {
        return Err(format!("{} is not a file", requested.display()));
    }
    let metadata =
        fs::metadata(&path).map_err(|error| format!("read {}: {error}", requested.display()))?;
    if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
        return Err(format!(
            "{} is larger than {MAX_WORKSPACE_FILE_BYTES} bytes",
            requested.display()
        ));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", requested.display()))?;
    let matches = content.matches(old).count();
    if matches == 0 {
        return Err(format!(
            "old string was not found in {}",
            requested.display()
        ));
    }
    if matches > 1 && !replace_all {
        return Err(format!(
            "old string matched {matches} times in {}; pass replace_all to change every occurrence, or include more surrounding context so the match is unique",
            requested.display()
        ));
    }
    let updated = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    write_file(&path, &updated)
        .map_err(|error| format!("write {}: {error}", requested.display()))?;
    Ok(WorkspaceEdit {
        path: relative_display(&root, &path, requested),
        replaced: matches,
        bytes: updated.len(),
    })
}

/// Result of [`edit_workspace_scoped`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceScopedEdit {
    /// Edited path, relative to the workspace root.
    pub path: String,
    /// Number of times `old` was replaced.
    pub replaced: usize,
    /// Byte length after the edit.
    pub bytes: usize,
    /// 1-based line of each replaced occurrence, in the file before the edit.
    pub lines: Vec<usize>,
    /// Name of the list entry (component, extension, dependency tuple) each
    /// replaced occurrence sits in; empty for top-level text.
    pub entries: Vec<String>,
    /// Set when one call changed several different entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A `('name', ...)` list entry: byte span and name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntrySpan {
    start: usize,
    end: usize,
    line: usize,
    name: String,
}

/// Tuple entries such as `('foo', '1.0', {...})` in `components`,
/// `exts_list` or `dependencies`: a line whose first non-blank text is `(`
/// followed by a quoted name and a comma. The span ends at the matching `)`.
fn entry_spans(text: &str) -> Vec<EntrySpan> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let lead = line.len() - line.trim_start().len();
        let rest = &line[lead..];
        if let Some(name) = entry_name(rest) {
            let start = offset + lead;
            if let Some(end) = matching_paren(text, start) {
                out.push(EntrySpan {
                    start,
                    end,
                    line: index + 1,
                    name,
                });
            }
        }
        offset += line.len();
    }
    out
}

fn entry_name(rest: &str) -> Option<String> {
    let inner = rest.strip_prefix('(')?.trim_start();
    let quote = inner.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let body = &inner[1..];
    let close = body.find(quote)?;
    let after = body[close + 1..].trim_start();
    (after.starts_with(',') && close > 0).then(|| body[..close].to_string())
}

/// Offset just past the `)` matching the `(` at `open`, skipping strings.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == b'\\' {
                    i += 1;
                } else if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'#' => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Innermost entry containing `offset`.
fn entry_at(spans: &[EntrySpan], offset: usize) -> Option<&EntrySpan> {
    spans
        .iter()
        .filter(|span| span.start <= offset && offset < span.end)
        .min_by_key(|span| span.end - span.start)
}

/// Like [`edit_workspace`], but able to address one list entry.
///
/// `within` names a `('name', ...)` entry (a component, extension or
/// dependency); only occurrences of `old` inside it are considered. When two
/// entries share the name, `occurrence` (1-based) picks one of them, else the
/// call fails and lists them. Without `within`, `occurrence` picks the n-th
/// match of `old`. An ambiguous match fails with the line and entry of every
/// occurrence, so near-identical components cannot be edited by accident; a
/// `replace_all` that changes several different entries succeeds and says
/// which ones in `note`.
pub fn edit_workspace_scoped(
    root: &Path,
    requested: &Path,
    old: &str,
    new: &str,
    replace_all: bool,
    within: Option<&str>,
    occurrence: Option<usize>,
) -> Result<WorkspaceScopedEdit, String> {
    if old.is_empty() {
        return Err("old string must not be empty".into());
    }
    if old == new {
        return Err("old and new are the same string".into());
    }
    let (root, path) = resolve_inside(root, requested)?;
    if !path.is_file() {
        return Err(format!("{} is not a file", requested.display()));
    }
    let metadata =
        fs::metadata(&path).map_err(|error| format!("read {}: {error}", requested.display()))?;
    if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
        return Err(format!(
            "{} is larger than {MAX_WORKSPACE_FILE_BYTES} bytes",
            requested.display()
        ));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", requested.display()))?;
    let spans = entry_spans(&content);
    let mut matches: Vec<usize> = content.match_indices(old).map(|(at, _)| at).collect();
    let describe = |at: usize| match entry_at(&spans, at) {
        Some(span) => format!("line {} (in '{}')", line_of(&content, at), span.name),
        None => format!("line {} (top level)", line_of(&content, at)),
    };
    let mut occurrence = occurrence;
    if let Some(name) = within {
        let mut named: Vec<&EntrySpan> = spans.iter().filter(|span| span.name == name).collect();
        if named.is_empty() {
            named = spans
                .iter()
                .filter(|span| span.name.eq_ignore_ascii_case(name))
                .collect();
        }
        let chosen = match (named.len(), occurrence.take()) {
            (0, _) => {
                let mut names: Vec<&str> = spans.iter().map(|span| span.name.as_str()).collect();
                names.dedup();
                return Err(format!(
                    "no entry named '{name}' in {}; entries: {}",
                    requested.display(),
                    names.join(", ")
                ));
            }
            (1, _) => named[0],
            (count, Some(k)) if k >= 1 && k <= count => named[k - 1],
            (count, _) => {
                let lines: Vec<String> = named.iter().map(|span| span.line.to_string()).collect();
                return Err(format!(
                    "'{name}' names {count} entries in {} (lines {}); pass occurrence (1-based) to pick the one the review is about",
                    requested.display(),
                    lines.join(", ")
                ));
            }
        };
        matches.retain(|at| {
            *at >= chosen.start
                && *at + old.len() <= chosen.end
                && entry_at(&spans, *at).is_some_and(|span| span.start >= chosen.start)
        });
        if matches.is_empty() {
            return Err(format!(
                "old string was not found inside '{name}' (line {}) in {}",
                chosen.line,
                requested.display()
            ));
        }
    }
    if matches.is_empty() {
        return Err(format!(
            "old string was not found in {}",
            requested.display()
        ));
    }
    if let Some(k) = occurrence {
        if k == 0 || k > matches.len() {
            return Err(format!(
                "occurrence {k} is out of range: old string matched {} time(s): {}",
                matches.len(),
                matches
                    .iter()
                    .map(|at| describe(*at))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        matches = vec![matches[k - 1]];
    }
    if matches.len() > 1 && !replace_all {
        return Err(format!(
            "old string matched {} times in {}: {}. Pass within with the entry name (and occurrence when names repeat) for the one the review is about, or include more surrounding context; pass replace_all only when the review asks for every occurrence",
            matches.len(),
            requested.display(),
            matches.iter().map(|at| describe(*at)).collect::<Vec<_>>().join(", ")
        ));
    }
    let lines: Vec<usize> = matches.iter().map(|at| line_of(&content, *at)).collect();
    let entries: Vec<String> = matches
        .iter()
        .map(|at| {
            entry_at(&spans, *at)
                .map(|span| span.name.clone())
                .unwrap_or_default()
        })
        .collect();
    let mut updated = String::with_capacity(content.len());
    let mut last = 0usize;
    for at in &matches {
        updated.push_str(&content[last..*at]);
        updated.push_str(new);
        last = at + old.len();
    }
    updated.push_str(&content[last..]);
    let mut distinct = entries.clone();
    distinct.sort();
    distinct.dedup();
    let note = (distinct.len() > 1).then(|| {
        format!(
            "changed {} different entries ({}); if the review is about one of them, undo the others and pass within",
            distinct.len(),
            distinct
                .iter()
                .map(|name| if name.is_empty() { "top level".to_string() } else { format!("'{name}'") })
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    write_file(&path, &updated)
        .map_err(|error| format!("write {}: {error}", requested.display()))?;
    Ok(WorkspaceScopedEdit {
        path: relative_display(&root, &path, requested),
        replaced: matches.len(),
        bytes: updated.len(),
        lines,
        entries,
        note,
    })
}

/// Result of [`replace_lines_workspace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceLineEdit {
    /// Edited path, relative to the workspace root.
    pub path: String,
    /// 1-based first and last line that were replaced (before the edit).
    pub replaced_lines: (usize, usize),
    /// The lines that were removed.
    pub removed: Vec<String>,
    /// Number of lines written in their place (0 deletes them).
    pub added: usize,
    /// True when `expect` was found at another place than the given lines
    /// (earlier edits moved them).
    pub relocated: bool,
    /// Byte length after the edit.
    pub bytes: usize,
}

/// Replace lines `start..=end` (1-based) with `replacement`, the way GitHub
/// applies a review suggestion to the lines its comment is anchored on: an
/// empty `replacement` deletes them. With `expect` (the anchored lines as
/// the review showed them), the lines must match it ignoring surrounding
/// whitespace; when they do not, a unique place where they do is used
/// instead, otherwise the call fails and nothing is written.
pub fn replace_lines_workspace(
    root: &Path,
    requested: &Path,
    start: usize,
    end: usize,
    replacement: &str,
    expect: Option<&str>,
) -> Result<WorkspaceLineEdit, String> {
    if start == 0 || end < start {
        return Err(format!(
            "bad line range {start}-{end}: lines are 1-based and end >= start"
        ));
    }
    let (root, path) = resolve_inside(root, requested)?;
    if !path.is_file() {
        return Err(format!("{} is not a file", requested.display()));
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", requested.display()))?;
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let norm =
        |text: &str| -> Vec<String> { text.lines().map(|line| line.trim().to_string()).collect() };
    let (mut first, mut last) = (start, end);
    let mut relocated = false;
    if let Some(expect) = expect.filter(|expect| !expect.trim().is_empty()) {
        let want = norm(expect.trim_matches('\n'));
        let have = |from: usize| -> Vec<String> {
            lines
                .iter()
                .skip(from - 1)
                .take(want.len())
                .map(|line| line.trim().to_string())
                .collect()
        };
        if end > lines.len() || have(start) != want {
            let found: Vec<usize> = (1..=lines.len().saturating_sub(want.len()) + 1)
                .filter(|from| have(*from) == want)
                .collect();
            match found.as_slice() {
                [only] => {
                    first = *only;
                    last = *only + want.len() - 1;
                    relocated = true;
                }
                [] => {
                    return Err(format!(
                        "lines {start}-{end} of {} do not hold the anchored text, and it was not found elsewhere; re-read the file",
                        requested.display()
                    ))
                }
                many => {
                    return Err(format!(
                        "lines {start}-{end} of {} do not hold the anchored text, which occurs at lines {}; pass the line range of the one the review is about",
                        requested.display(),
                        many.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
                    ))
                }
            }
        } else {
            last = start + want.len() - 1;
        }
    }
    if last > lines.len() {
        return Err(format!(
            "line range {first}-{last} is past the end of {} ({} lines)",
            requested.display(),
            lines.len()
        ));
    }
    let removed: Vec<String> = lines[first - 1..last]
        .iter()
        .map(|line| line.trim_end_matches('\n').to_string())
        .collect();
    let mut updated = String::with_capacity(content.len() + replacement.len());
    for line in &lines[..first - 1] {
        updated.push_str(line);
    }
    let body = replacement.trim_end_matches('\n');
    let added = if body.is_empty() && replacement.trim().is_empty() {
        0
    } else {
        updated.push_str(body);
        updated.push('\n');
        body.lines().count()
    };
    for line in &lines[last..] {
        updated.push_str(line);
    }
    write_file(&path, &updated)
        .map_err(|error| format!("write {}: {error}", requested.display()))?;
    Ok(WorkspaceLineEdit {
        path: relative_display(&root, &path, requested),
        replaced_lines: (first, last),
        removed,
        added,
        relocated,
        bytes: updated.len(),
    })
}

/// Create or overwrite a UTF-8 file inside `root`.
///
/// Parent directories must already exist. The call does not follow a symlink
/// that leaves the workspace.
pub fn write_workspace(
    root: &Path,
    requested: &Path,
    content: &str,
) -> Result<WorkspaceWrite, String> {
    if content.len() as u64 > MAX_WORKSPACE_FILE_BYTES {
        return Err(format!(
            "content is larger than {MAX_WORKSPACE_FILE_BYTES} bytes"
        ));
    }
    let (root, path) = resolve_inside(root, requested)?;
    if path.is_dir() {
        return Err(format!("{} is a directory", requested.display()));
    }
    let created = !path.exists();
    write_file(&path, content)
        .map_err(|error| format!("write {}: {error}", requested.display()))?;
    Ok(WorkspaceWrite {
        path: relative_display(&root, &path, requested),
        created,
        bytes: content.len(),
    })
}

fn write_file(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.flush()
}

fn relative_display(root: &Path, resolved: &Path, requested: &Path) -> String {
    if requested.is_absolute() {
        resolved
            .strip_prefix(root)
            .unwrap_or(resolved)
            .display()
            .to_string()
    } else {
        requested.display().to_string()
    }
}

fn symlink_escapes(root: &Path, path: &Path) -> bool {
    match path.canonicalize() {
        Ok(target) => !target.starts_with(root),
        Err(_) => true,
    }
}

fn resolve_inside(root: &Path, requested: &Path) -> Result<(PathBuf, PathBuf), String> {
    if requested.as_os_str().is_empty() {
        return Err("path is empty".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("workspace root {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!(
            "workspace root {} is not a directory",
            root.display()
        ));
    }
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let normalized = lexical_normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(format!(
            "path escapes the workspace root: {}",
            requested.display()
        ));
    }
    let relative = normalized
        .strip_prefix(&root)
        .map_err(|_| format!("path escapes the workspace root: {}", requested.display()))?;
    let mut current = root.clone();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(format!(
                "path escapes the workspace root: {}",
                requested.display()
            ));
        };
        current.push(name);
        let last = index + 1 == components.len();
        if current.is_symlink() {
            let target = current.canonicalize().map_err(|_| {
                format!(
                    "symlink escapes the workspace root: {}",
                    requested.display()
                )
            })?;
            if !target.starts_with(&root) {
                return Err(format!(
                    "symlink escapes the workspace root: {}",
                    requested.display()
                ));
            }
            current = target;
        } else if last {
            if let Some(parent) = current.parent() {
                if !parent.is_dir() || !parent.starts_with(&root) {
                    return Err(format!(
                        "parent directory does not exist inside the workspace: {}",
                        requested.display()
                    ));
                }
            }
        } else if !current.is_dir() {
            return Err(format!("not a directory: {}", requested.display()));
        }
        if !current.starts_with(&root) {
            return Err(format!(
                "path escapes the workspace root: {}",
                requested.display()
            ));
        }
    }
    Ok((root, current))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_root<T>(root: &Path, body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().expect("workspace env lock");
        let previous = std::env::var_os("EB_STACK_WORKSPACE");
        std::env::set_var("EB_STACK_WORKSPACE", root);
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => std::env::set_var("EB_STACK_WORKSPACE", value),
                    None => std::env::remove_var("EB_STACK_WORKSPACE"),
                }
            }
        }
        let _restore = Restore(previous);
        body()
    }

    #[test]
    fn configured_root_prefers_the_environment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let found = with_env_root(temp.path(), || configured_workspace_root().expect("root"));
        assert_eq!(found, temp.path());
    }

    #[test]
    fn read_write_edit_and_list_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let written =
            write_workspace(root, Path::new("demo.eb"), "name = 'demo'\n").expect("write");
        assert!(written.created);
        let text = read_workspace(root, Path::new("demo.eb")).expect("read");
        assert_eq!(text.content, "name = 'demo'\n");
        let edited =
            edit_workspace(root, Path::new("demo.eb"), "demo", "p7zip", false).expect("edit");
        assert_eq!(edited.replaced, 1);
        assert_eq!(
            read_workspace(root, Path::new("demo.eb"))
                .expect("reread")
                .content,
            "name = 'p7zip'\n"
        );
        let listing = list_workspace(root, Path::new(".")).expect("list");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "demo.eb");
        assert_eq!(listing.entries[0].kind, "file");
        assert!(!listing.entries[0].escapes);
    }

    #[test]
    fn grep_finds_literal_and_regex_matches_inside_the_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::create_dir_all(root.join("easybuild/easyconfigs/p")).expect("mkdir");
        fs::create_dir_all(root.join(".git")).expect("git dir");
        fs::write(root.join(".git/config"), "SHLIB_EXT\n").expect("git file");
        fs::write(
            root.join("easybuild/easyconfigs/p/p.eb"),
            "files = ['lib/libp.%s' % SHLIB_EXT]\nother = f'lib/%(name)s.{SHLIB_EXT}'\n",
        )
        .expect("recipe");

        let literal =
            grep_workspace(root, Path::new("."), "% SHLIB_EXT", false, None).expect("literal grep");
        assert_eq!(literal.matches.len(), 1, "{literal:?}");
        assert_eq!(literal.matches[0].line, 1);
        assert_eq!(literal.matches[0].path, "easybuild/easyconfigs/p/p.eb");

        let regex = grep_workspace(root, Path::new("easybuild"), r"SHLIB_EXT\b", true, None)
            .expect("regex grep");
        assert_eq!(regex.matches.len(), 2, "{regex:?}");
        assert!(!regex.truncated);

        let limited =
            grep_workspace(root, Path::new("."), "SHLIB_EXT", false, Some(1)).expect("limit");
        assert_eq!(limited.matches.len(), 1);
        assert!(limited.truncated);

        assert!(grep_workspace(root, Path::new("../"), "x", false, None).is_err());
        assert!(grep_workspace(root, Path::new("."), "(", true, None).is_err());
    }

    #[test]
    fn repeated_match_requires_replace_all() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let source = "a = '%s' % SHLIB_EXT\nb = '%s' % SHLIB_EXT\n";
        write_workspace(root, Path::new("twice.eb"), source).expect("write");
        let error = edit_workspace(
            root,
            Path::new("twice.eb"),
            "'%s' % SHLIB_EXT",
            "f'{SHLIB_EXT}'",
            false,
        )
        .expect_err("ambiguous edit");
        assert!(error.contains("replace_all"), "{error}");
        assert_eq!(
            read_workspace(root, Path::new("twice.eb"))
                .expect("unchanged")
                .content,
            source
        );
        let edited = edit_workspace(
            root,
            Path::new("twice.eb"),
            "'%s' % SHLIB_EXT",
            "f'{SHLIB_EXT}'",
            true,
        )
        .expect("replace all");
        assert_eq!(edited.replaced, 2);
        let updated = read_workspace(root, Path::new("twice.eb"))
            .expect("read")
            .content;
        assert_eq!(updated.matches("f'{SHLIB_EXT}'").count(), 2);
        assert!(!updated.contains("% SHLIB_EXT"));
    }

    #[test]
    fn rejects_parent_and_absolute_escapes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_workspace(root, Path::new("inside.eb"), "ok\n").expect("write");
        for path in ["../outside.eb", "/etc/passwd", "sub/../../outside.eb"] {
            let error = read_workspace(root, Path::new(path)).expect_err(path);
            assert!(error.contains("escapes"), "{path}: {error}");
        }
        let inside = root.join("inside.eb");
        assert_eq!(
            read_workspace(root, &inside)
                .expect("absolute inside")
                .content,
            "ok\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_leave_the_workspace() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        let root = temp.path();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "original").expect("secret");
        std::os::unix::fs::symlink(&secret, root.join("leak")).expect("symlink");
        fs::create_dir(root.join("nested")).expect("nested");
        std::os::unix::fs::symlink(outside.path(), root.join("nested").join("out"))
            .expect("dir symlink");

        let read_error = read_workspace(root, Path::new("leak")).expect_err("read leak");
        assert!(read_error.contains("symlink escapes"), "{read_error}");
        assert!(!read_error.contains("original"));
        let edit_error = edit_workspace(root, Path::new("leak"), "original", "pwned", false)
            .expect_err("edit leak");
        assert!(edit_error.contains("symlink escapes"), "{edit_error}");
        assert_eq!(
            fs::read_to_string(&secret).expect("secret unchanged"),
            "original"
        );
        let write_error = write_workspace(root, Path::new("nested/out/planted"), "pwned")
            .expect_err("write through");
        assert!(write_error.contains("symlink escapes"), "{write_error}");
        assert!(!outside.path().join("planted").exists());

        let listing = list_workspace(root, Path::new(".")).expect("list");
        let leak = listing
            .entries
            .iter()
            .find(|entry| entry.name == "leak")
            .expect("leak entry");
        assert_eq!(leak.kind, "symlink");
        assert!(leak.escapes);

        fs::write(root.join("ok.eb"), "name = 'ok'\n").expect("ok");
        std::os::unix::fs::symlink("ok.eb", root.join("alias.eb")).expect("inside symlink");
        assert_eq!(
            read_workspace(root, Path::new("alias.eb"))
                .expect("inside symlink read")
                .content,
            "name = 'ok'\n"
        );
    }

    #[test]
    fn scoped_edit_addresses_one_of_two_near_identical_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let text = "components = [\n    ('alpha', '1.0', {\n        'skip': True,\n    }),\n    ('beta', '1.0', {\n        'skip': True,\n    }),\n]\n";
        fs::write(root.join("x.eb"), text).expect("write");
        let ambiguous = edit_workspace_scoped(
            root,
            Path::new("x.eb"),
            "'skip': True,",
            "'skip': True,\n        'opt': 1,",
            false,
            None,
            None,
        )
        .expect_err("ambiguous");
        assert!(
            ambiguous.contains("line 3 (in 'alpha')") && ambiguous.contains("line 6 (in 'beta')"),
            "{ambiguous}"
        );
        let edited = edit_workspace_scoped(
            root,
            Path::new("x.eb"),
            "'skip': True,",
            "'skip': True,\n        'opt': 1,",
            false,
            Some("beta"),
            None,
        )
        .expect("edit");
        assert_eq!(edited.lines, [6]);
        assert_eq!(edited.entries, ["beta"]);
        let after = fs::read_to_string(root.join("x.eb")).expect("read");
        assert!(
            after.contains("('beta', '1.0', {\n        'skip': True,\n        'opt': 1,"),
            "{after}"
        );
        assert!(
            !after.contains("('alpha', '1.0', {\n        'skip': True,\n        'opt'"),
            "{after}"
        );
        let all =
            edit_workspace_scoped(root, Path::new("x.eb"), "'1.0'", "'2.0'", true, None, None)
                .expect("all");
        assert!(
            all.note
                .as_deref()
                .is_some_and(|note| note.contains("2 different entries")),
            "{all:?}"
        );
    }

    #[test]
    fn line_suggestion_deletes_or_replaces_the_anchored_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("y.eb"), "a = 1\nb = 2\nc = 3\nd = 4\n").expect("write");
        let deleted = replace_lines_workspace(root, Path::new("y.eb"), 2, 2, "", Some("b = 2"))
            .expect("delete");
        assert_eq!(deleted.removed, ["b = 2"]);
        assert_eq!(
            fs::read_to_string(root.join("y.eb")).expect("read"),
            "a = 1\nc = 3\nd = 4\n"
        );
        // Anchored text moved up by one line: found and replaced there.
        let moved = replace_lines_workspace(root, Path::new("y.eb"), 3, 3, "c = 30", Some("c = 3"))
            .expect("moved");
        assert!(moved.relocated);
        assert_eq!(
            fs::read_to_string(root.join("y.eb")).expect("read"),
            "a = 1\nc = 30\nd = 4\n"
        );
        assert!(replace_lines_workspace(root, Path::new("y.eb"), 1, 1, "", Some("zzz")).is_err());
    }
}
