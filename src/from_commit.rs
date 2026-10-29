//! Classify easystack `from-commit` pins against an EasyBuild release.
//!
//! `stack from-commit` reads easystacks, the documents `stack easystack`
//! writes. `recipe lint` checks `.eb` text, and a pin is not recipe text,
//! so the check is a stack subcommand. Containment is `git merge-base
//! --is-ancestor`, the same question the reach and overlap tools ask of a
//! commit and a ref: is this commit already in the release tag.

use crate::cli_parse::usage;
use crate::easystack::parse_easystack_entries;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Arguments for `stack from-commit`.
pub struct FromCommitRequest {
    /// Easystack files, in report order.
    pub easystacks: Vec<PathBuf>,
    /// easybuild-easyconfigs checkout for `from-commit` pins.
    pub easyconfigs_repo: Option<PathBuf>,
    /// easybuild-easyblocks checkout for `include-easyblocks-from-commit` pins.
    pub easyblocks_repo: Option<PathBuf>,
    /// Release `X.Y.Z`. Overrides `eb-X.Y.Z` in each easystack file name.
    pub easybuild_version: Option<String>,
    /// Print the report as JSON.
    pub json: bool,
    /// Drop each pin the release already carries with the same bytes, in
    /// place. A pin whose released file differs stays: it names the file
    /// that was installed, and the release would build a different one.
    pub rewrite: bool,
}

/// The report `stack from-commit` prints.
#[derive(Debug)]
pub struct FromCommitReport {
    /// CLI stdout, including the trailing newline.
    pub stdout: String,
    /// The same report as JSON.
    pub document: Value,
    /// Pins whose commit is contained in the release tag.
    pub released: usize,
    /// Pins whose commit is in the checkout and is not in the release tag.
    pub newer: usize,
    /// Pins whose commit or release tag is missing from the checkout.
    pub unknown: usize,
    /// Pins `--rewrite` dropped from the easystacks.
    pub rewritten: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Easyconfigs,
    Easyblocks,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Easyconfigs => "from-commit",
            Self::Easyblocks => "include-easyblocks-from-commit",
        }
    }

    fn tag(self, version: &str) -> String {
        let project = match self {
            Self::Easyconfigs => "easybuild-easyconfigs",
            Self::Easyblocks => "easybuild-easyblocks",
        };
        format!("{project}-v{version}")
    }
}

struct RawPin {
    easystack: String,
    file: String,
    kind: Kind,
    commit: String,
    version: String,
}

struct PinRow {
    easystack: String,
    file: String,
    kind: String,
    commit: String,
    version: String,
    tag: String,
    status: String,
    blob: Option<String>,
    reason: Option<String>,
}

/// Classify every `from-commit` and `include-easyblocks-from-commit` pin.
///
/// Exit status is not applied here. The CLI exits 1 when [`FromCommitReport::released`]
/// is non-zero. `unknown` is a missing commit or a missing tag, never a guess.
pub fn check(request: &FromCommitRequest) -> Result<FromCommitReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    let version_flag = match &request.easybuild_version {
        Some(version) if is_release_version(version) => Some(version.as_str()),
        Some(version) => {
            return Err(usage(format!(
                "--easybuild-version wants X.Y.Z, got {version}"
            )));
        }
        None => None,
    };
    let mut raw = Vec::new();
    for path in &request.easystacks {
        raw.extend(read_pins(path, version_flag)?);
    }
    let needs_easyconfigs = raw.iter().any(|pin| pin.kind == Kind::Easyconfigs);
    let needs_easyblocks = raw.iter().any(|pin| pin.kind == Kind::Easyblocks);
    if needs_easyconfigs {
        let repo = request
            .easyconfigs_repo
            .as_deref()
            .ok_or_else(|| usage("--easyconfigs-repo is required to check from-commit pins"))?;
        require_repo(repo, "--easyconfigs-repo")?;
    }
    if needs_easyblocks {
        let repo = request.easyblocks_repo.as_deref().ok_or_else(|| {
            usage("--easyblocks-repo is required to check include-easyblocks-from-commit pins")
        })?;
        require_repo(repo, "--easyblocks-repo")?;
    }
    let mut cache: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut rows = Vec::with_capacity(raw.len());
    for pin in &raw {
        let repo = match pin.kind {
            Kind::Easyconfigs => request.easyconfigs_repo.as_deref(),
            Kind::Easyblocks => request.easyblocks_repo.as_deref(),
        };
        let Some(repo) = repo else {
            bail!("internal: {} has no checkout", pin.kind.as_str());
        };
        rows.push(classify(repo, pin, &mut cache)?);
    }
    let mut rewritten = 0;
    if request.rewrite {
        for path in &request.easystacks {
            let shown = path.display().to_string();
            let droppable: Vec<ReleasedPin> = rows
                .iter()
                .filter(|row| {
                    row.easystack == shown
                        && row.status == "released"
                        && row.blob.as_deref() == Some("same")
                })
                .map(|row| ReleasedPin {
                    file: row.file.clone(),
                    key: if row.kind == Kind::Easyblocks.as_str() {
                        Kind::Easyblocks
                    } else {
                        Kind::Easyconfigs
                    },
                    commit: row.commit.clone(),
                })
                .collect();
            if droppable.is_empty() {
                continue;
            }
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("--easystack {}", path.display()))?;
            let (updated, dropped) = drop_pins(&text, &droppable);
            std::fs::write(path, updated).with_context(|| format!("write {}", path.display()))?;
            rewritten += dropped;
        }
        for row in &mut rows {
            if row.status == "released" && row.blob.as_deref() == Some("same") {
                row.status = "dropped".into();
            }
        }
    }
    let mut report = render(rows, request.json)?;
    report.rewritten = rewritten;
    if request.rewrite && !request.json {
        report.stdout.push_str(&format!("rewritten={rewritten}\n"));
    }
    if request.rewrite {
        report.document["rewritten"] = json!(rewritten);
    }
    Ok(report)
}

/// One pin `--rewrite` removes: the entry's file, the option, and its commit.
struct ReleasedPin {
    file: String,
    key: Kind,
    commit: String,
}

/// Remove the named pins from easystack text, keeping every other line.
///
/// An entry is `  - FILE:` followed by its more-indented option lines. A pin
/// line goes only when its option and commit both match, so an entry that
/// repeats a file with another pin keeps that pin. The comment lines
/// directly above a removed pin go with it. When nothing but `options:` and
/// comments is left, the entry collapses to `  - FILE`. Returns the new text
/// and the pins dropped.
fn drop_pins(text: &str, drop: &[ReleasedPin]) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut dropped = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let entry = line
            .trim_start()
            .strip_prefix("- ")
            .and_then(|rest| rest.trim_end().strip_suffix(':'))
            .map(str::trim);
        let indent = line.len() - line.trim_start().len();
        let Some(file) = entry.filter(|file| drop.iter().any(|d| d.file == *file)) else {
            out.push(line.to_string());
            i += 1;
            continue;
        };
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let next_indent = next.len() - next.trim_start().len();
            if next.trim().is_empty() || next_indent <= indent {
                break;
            }
            j += 1;
        }
        let pins: Vec<(&str, &str)> = drop
            .iter()
            .filter(|d| d.file == file)
            .map(|d| (d.key.as_str(), d.commit.as_str()))
            .collect();
        let mut block: Vec<&str> = lines[i + 1..j].to_vec();
        let mut k = 0;
        while k < block.len() {
            let (key, value) = block[k]
                .trim_start()
                .split_once(':')
                .map(|(k, v)| (k.trim(), v.trim()))
                .unwrap_or(("", ""));
            if pins.contains(&(key, value)) {
                let mut start = k;
                while start > 0 && block[start - 1].trim_start().starts_with('#') {
                    start -= 1;
                }
                block.drain(start..=k);
                dropped += 1;
                k = start;
            } else {
                k += 1;
            }
        }
        let has_option = block.iter().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && t != "options:"
        });
        if has_option {
            out.push(line.to_string());
            out.extend(block.iter().map(|l| l.to_string()));
        } else {
            out.push(format!("{}- {file}", " ".repeat(indent)));
        }
        i = j;
    }
    let mut text_out = out.join("\n");
    if text.ends_with('\n') {
        text_out.push('\n');
    }
    (text_out, dropped)
}

fn read_pins(path: &Path, version_flag: Option<&str>) -> Result<Vec<RawPin>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("--easystack {}", path.display()))?;
    let entries = parse_easystack_entries(&text)
        .with_context(|| format!("parsing easystack {}", path.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        if let Some(commit) = entry.from_commit {
            found.push((Kind::Easyconfigs, entry.file.clone(), commit));
        }
        if let Some(commit) = entry.include_easyblocks_from_commit {
            found.push((Kind::Easyblocks, entry.file, commit));
        }
    }
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let version = resolve_version(path, version_flag)?;
    let easystack = path.display().to_string();
    Ok(found
        .into_iter()
        .map(|(kind, file, commit)| RawPin {
            easystack: easystack.clone(),
            file,
            kind,
            commit,
            version: version.clone(),
        })
        .collect())
}

fn resolve_version(path: &Path, version_flag: Option<&str>) -> Result<String> {
    if let Some(version) = version_flag {
        return Ok(version.to_string());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if let Some(version) = version_in_file_name(name) {
        return Ok(version);
    }
    Err(usage(format!(
        "pass --easybuild-version X.Y.Z: easystack {} has no eb-X.Y.Z in its file name",
        path.display()
    )))
}

/// `eb-X.Y.Z` in a file name, not as the tail of a longer token such as `web-`.
fn version_in_file_name(name: &str) -> Option<String> {
    let mut rest = name;
    while let Some(index) = rest.find("eb-") {
        let boundary = index == 0 || !rest.as_bytes()[index - 1].is_ascii_alphanumeric();
        let after = &rest[index + 3..];
        if boundary {
            if let Some(version) = leading_release_version(after) {
                return Some(version);
            }
        }
        rest = after;
    }
    None
}

fn is_release_version(text: &str) -> bool {
    leading_release_version(text).as_deref() == Some(text)
}

/// The leading `X.Y.Z` of `text`, when one is there.
fn leading_release_version(text: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut number = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        if character == '.' && !number.is_empty() && parts.len() < 2 {
            parts.push(std::mem::take(&mut number));
            continue;
        }
        break;
    }
    if number.is_empty() || parts.len() != 2 {
        return None;
    }
    parts.push(number);
    Some(parts.join("."))
}

fn require_repo(path: &Path, flag: &str) -> Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("{flag} {}", path.display()))?;
    if !meta.is_dir() {
        bail!("{flag} {} is not a directory", path.display());
    }
    let out = git(path, &["rev-parse", "--is-inside-work-tree"])?;
    if !out.status.success() || std::str::from_utf8(&out.stdout).unwrap_or("").trim() != "true" {
        bail!("{flag} {} is not a git checkout", path.display());
    }
    Ok(())
}

fn classify(
    repo: &Path,
    pin: &RawPin,
    cache: &mut HashMap<String, Vec<(String, String)>>,
) -> Result<PinRow> {
    let tag = pin.kind.tag(&pin.version);
    let mut row = PinRow {
        easystack: pin.easystack.clone(),
        file: pin.file.clone(),
        kind: pin.kind.as_str().to_string(),
        commit: pin.commit.clone(),
        version: pin.version.clone(),
        tag: tag.clone(),
        status: String::new(),
        blob: None,
        reason: None,
    };
    if !object_exists(repo, &format!("{tag}^{{commit}}"))? {
        row.status = "unknown".into();
        row.reason = Some("tag-missing".into());
        return Ok(row);
    }
    let pin_spec = format!("{}^{{commit}}", pin.commit);
    if !rev_is_safe(&pin.commit) || !object_exists(repo, &pin_spec)? {
        row.status = "unknown".into();
        row.reason = Some("commit-missing".into());
        return Ok(row);
    }
    if !is_ancestor(repo, &pin.commit, &tag)? {
        row.status = "newer".into();
        return Ok(row);
    }
    row.status = "released".into();
    row.blob = Some(blob_relation(repo, pin, &tag, cache)?);
    Ok(row)
}

/// A commit id we can pass to git without it being read as a flag.
fn rev_is_safe(rev: &str) -> bool {
    !rev.is_empty()
        && rev.len() <= 128
        && !rev.starts_with('-')
        && rev
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn blob_relation(
    repo: &Path,
    pin: &RawPin,
    tag: &str,
    cache: &mut HashMap<String, Vec<(String, String)>>,
) -> Result<String> {
    let same = match pin.kind {
        Kind::Easyconfigs => {
            let left = file_blob(cache, repo, &pin.commit, &pin.file)?;
            let right = file_blob(cache, repo, tag, &pin.file)?;
            left.is_some() && left == right
        }
        Kind::Easyblocks => {
            let left = tree_id(repo, &pin.commit)?;
            let right = tree_id(repo, tag)?;
            left.is_some() && left == right
        }
    };
    Ok(if same { "same" } else { "differs" }.into())
}

fn file_blob(
    cache: &mut HashMap<String, Vec<(String, String)>>,
    repo: &Path,
    rev: &str,
    filename: &str,
) -> Result<Option<String>> {
    let key = format!("{}\0{rev}", repo.display());
    if !cache.contains_key(&key) {
        let listed = list_blobs(repo, rev)?;
        cache.insert(key.clone(), listed);
    }
    let Some(listed) = cache.get(&key) else {
        return Ok(None);
    };
    let mut hashes = listed
        .iter()
        .filter(|(_, path)| path == filename || path.ends_with(&format!("/{filename}")))
        .map(|(hash, _)| hash.as_str());
    let Some(first) = hashes.next() else {
        return Ok(None);
    };
    if hashes.next().is_some() {
        return Ok(None);
    }
    Ok(Some(first.to_string()))
}

/// Blob hashes named by the tree. A blobless checkout still has these.
fn list_blobs(repo: &Path, rev: &str) -> Result<Vec<(String, String)>> {
    let text = git_stdout(repo, &["ls-tree", "-r", rev])?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let _mode = parts.next();
        let kind = parts.next();
        let hash = parts.next();
        if kind == Some("blob") {
            if let Some(hash) = hash {
                out.push((hash.to_string(), path.to_string()));
            }
        }
    }
    Ok(out)
}

fn tree_id(repo: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{tree}}");
    let out = git(repo, &["rev-parse", "--verify", "--quiet", &spec])?;
    if !out.status.success() {
        return Ok(None);
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}

fn object_exists(repo: &Path, spec: &str) -> Result<bool> {
    let out = git(repo, &["rev-parse", "--verify", "--quiet", spec])?;
    if out.status.success() {
        return Ok(true);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if err.is_empty()
        || err.contains("Needed a single revision")
        || err.contains("unknown revision")
        || err.contains("not a valid object")
        || err.contains("Not a valid object")
        || err.contains("ambiguous argument")
        || err.contains("bad revision")
    {
        return Ok(false);
    }
    bail!("git rev-parse {spec} in {}: {err}", repo.display());
}

fn is_ancestor(repo: &Path, pin: &str, tag: &str) -> Result<bool> {
    let out = git(repo, &["merge-base", "--is-ancestor", pin, tag])?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git merge-base --is-ancestor {pin} {tag} in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running git {} in {}", args.join(" "), repo.display()))
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git(repo, args)?;
    if !out.status.success() {
        bail!(
            "git {} in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn render(rows: Vec<PinRow>, as_json: bool) -> Result<FromCommitReport> {
    let released = rows.iter().filter(|row| row.status == "released").count();
    let newer = rows.iter().filter(|row| row.status == "newer").count();
    let unknown = rows.iter().filter(|row| row.status == "unknown").count();
    let pins: Vec<Value> = rows.iter().map(pin_value).collect();
    let document = json!({
        "pins": pins,
        "released": released,
        "newer": newer,
        "unknown": unknown,
    });
    let stdout = if as_json {
        format!("{}\n", serde_json::to_string_pretty(&document)?)
    } else {
        let mut text = String::new();
        for row in &rows {
            text.push_str(&human_line(row));
            text.push('\n');
        }
        text.push_str(&format!(
            "pins={} released={released} newer={newer} unknown={unknown}\n",
            rows.len()
        ));
        text
    };
    Ok(FromCommitReport {
        stdout,
        document,
        released,
        newer,
        unknown,
        rewritten: 0,
    })
}

fn pin_value(row: &PinRow) -> Value {
    let mut value = json!({
        "easystack": row.easystack,
        "file": row.file,
        "kind": row.kind,
        "commit": row.commit,
        "version": row.version,
        "tag": row.tag,
        "status": row.status,
    });
    if let Some(blob) = &row.blob {
        value["blob"] = json!(blob);
    }
    if let Some(reason) = &row.reason {
        value["reason"] = json!(reason);
    }
    value
}

fn human_line(row: &PinRow) -> String {
    let mut line = format!("status={}", row.status);
    if let Some(blob) = &row.blob {
        line.push_str(&format!(" blob={blob}"));
    }
    if let Some(reason) = &row.reason {
        line.push_str(&format!(" reason={reason}"));
    }
    line.push_str(&format!(
        " version={} tag={} kind={} commit={} file={} easystack={}",
        row.version, row.tag, row.kind, row.commit, row.file, row.easystack
    ));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Fixture {
        _keep: tempfile::TempDir,
        easyconfigs: PathBuf,
        easyblocks: PathBuf,
        c1: String,
        c2: String,
        c3: String,
        b0: String,
        b1: String,
        dir: PathBuf,
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .arg("-c")
            .arg("commit.gpgsign=false")
            .arg("-c")
            .arg("user.name=Rohit Goswami")
            .arg("-c")
            .arg("user.email=rgoswami@ieee.org")
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn commit_all(dir: &Path, message: &str) -> String {
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn fixture() -> Fixture {
        let keep = tempfile::tempdir().unwrap();
        let root = keep.path();
        let easyconfigs = root.join("easyconfigs");
        let easyblocks = root.join("easyblocks");
        std::fs::create_dir_all(&easyconfigs).unwrap();
        std::fs::create_dir_all(&easyblocks).unwrap();
        git(&easyconfigs, &["init", "-b", "main"]);
        write(&easyconfigs, "README", "root\n");
        let _c0 = commit_all(&easyconfigs, "root");
        git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.0.0"]);
        write(
            &easyconfigs,
            "easybuild/easyconfigs/f/Foo/Foo-1.0.eb",
            "one\n",
        );
        write(
            &easyconfigs,
            "easybuild/easyconfigs/b/Bar/Bar-1.0.eb",
            "old\n",
        );
        let c1 = commit_all(&easyconfigs, "add Foo and Bar");
        write(
            &easyconfigs,
            "easybuild/easyconfigs/b/Bar/Bar-1.0.eb",
            "new\n",
        );
        let c2 = commit_all(&easyconfigs, "patch Bar");
        git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.4.0"]);
        write(
            &easyconfigs,
            "easybuild/easyconfigs/n/New/New-1.0.eb",
            "after\n",
        );
        let c3 = commit_all(&easyconfigs, "add New");

        git(&easyblocks, &["init", "-b", "main"]);
        write(&easyblocks, "README", "blocks\n");
        let b0 = commit_all(&easyblocks, "easyblocks root");
        write(
            &easyblocks,
            "easybuild/easyblocks/generic/foo.py",
            "class Foo\n",
        );
        let b1 = commit_all(&easyblocks, "add foo");
        git(&easyblocks, &["tag", "easybuild-easyblocks-v5.4.0"]);

        Fixture {
            dir: root.to_path_buf(),
            _keep: keep,
            easyconfigs,
            easyblocks,
            c1,
            c2,
            c3,
            b0,
            b1,
        }
    }

    fn write_stack(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn request(
        stacks: &[PathBuf],
        fixture: &Fixture,
        version: Option<&str>,
        json: bool,
    ) -> FromCommitRequest {
        FromCommitRequest {
            easystacks: stacks.to_vec(),
            easyconfigs_repo: Some(fixture.easyconfigs.clone()),
            easyblocks_repo: Some(fixture.easyblocks.clone()),
            easybuild_version: version.map(str::to_string),
            json,
            rewrite: false,
        }
    }

    fn statuses(report: &FromCommitReport) -> Vec<String> {
        report.document["pins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pin| {
                format!(
                    "{} {} {} {}",
                    pin["kind"].as_str().unwrap(),
                    pin["file"].as_str().unwrap(),
                    pin["status"].as_str().unwrap(),
                    pin["blob"]
                        .as_str()
                        .unwrap_or(pin["reason"].as_str().unwrap_or("-"))
                )
            })
            .collect()
    }

    #[test]
    fn classifies_released_same_differs_newer_unknown_and_easyblocks() {
        let fixture = fixture();
        let missing = "0123456789abcdef0123456789abcdef01234567";
        let demo = write_stack(
            &fixture.dir,
            "20261011-eb-5.4.0-UCX.yml",
            &format!(
                "easyconfigs:\n  - zlib-1.2.13.eb\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n  - Bar-1.0.eb:\n      options:\n        from-commit: {c1}\n  - New-1.0.eb:\n      options:\n        from-commit: {c3}\n  - Foo-1.0.eb:\n      options:\n        from-commit: {missing}\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c2}\n        include-easyblocks-from-commit: {b1}\n",
                c1 = fixture.c1,
                c3 = fixture.c3,
                c2 = fixture.c2,
                b1 = fixture.b1,
            ),
        );
        let blocks = write_stack(
            &fixture.dir,
            "20261011-eb-5.4.0-blocks.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        include-easyblocks-from-commit: {b0}\n",
                b0 = fixture.b0,
            ),
        );
        let missing_tag = write_stack(
            &fixture.dir,
            "20261011-eb-9.9.9-missing.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n",
                c1 = fixture.c1,
            ),
        );
        let report = check(&request(
            &[demo.clone(), blocks.clone(), missing_tag.clone()],
            &fixture,
            None,
            false,
        ))
        .unwrap();
        assert_eq!(
            statuses(&report),
            vec![
                "from-commit Foo-1.0.eb released same".to_string(),
                "from-commit Bar-1.0.eb released differs".to_string(),
                "from-commit New-1.0.eb newer -".to_string(),
                "from-commit Foo-1.0.eb unknown commit-missing".to_string(),
                "from-commit Foo-1.0.eb released same".to_string(),
                "include-easyblocks-from-commit Foo-1.0.eb released same".to_string(),
                "include-easyblocks-from-commit Foo-1.0.eb released differs".to_string(),
                "from-commit Foo-1.0.eb unknown tag-missing".to_string(),
            ]
        );
        assert_eq!(report.released, 5);
        assert_eq!(report.newer, 1);
        assert_eq!(report.unknown, 2);
        assert!(report.stdout.contains("version=5.4.0"));
        assert!(report.stdout.contains("version=9.9.9"));
        assert!(report
            .stdout
            .contains(&format!("easystack={}", demo.display())));
        assert!(report
            .stdout
            .lines()
            .next_back()
            .unwrap()
            .contains("pins=8 released=5 newer=1 unknown=2"));
        let as_json = check(&request(&[demo], &fixture, None, true)).unwrap();
        let parsed: Value = serde_json::from_str(as_json.stdout.trim()).unwrap();
        assert_eq!(parsed["released"], 4);
        assert_eq!(parsed["pins"][0]["tag"], "easybuild-easyconfigs-v5.4.0");
    }

    #[test]
    fn version_flag_overrides_the_file_name() {
        let fixture = fixture();
        let named = write_stack(
            &fixture.dir,
            "20261011-eb-5.4.0-UCX.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n",
                c1 = fixture.c1,
            ),
        );
        let from_name = check(&request(
            std::slice::from_ref(&named),
            &fixture,
            None,
            false,
        ))
        .unwrap();
        assert_eq!(from_name.released, 1, "{}", from_name.stdout);
        assert!(from_name.stdout.contains("version=5.4.0"));
        let overridden = check(&request(&[named], &fixture, Some("5.0.0"), false)).unwrap();
        assert_eq!(overridden.released, 0, "{}", overridden.stdout);
        assert_eq!(overridden.newer, 1);
        assert!(overridden.stdout.contains("version=5.0.0"));
        assert!(overridden
            .stdout
            .contains("tag=easybuild-easyconfigs-v5.0.0"));

        let other_name = write_stack(
            &fixture.dir,
            "20261011-eb-9.9.9-missing.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n",
                c1 = fixture.c1,
            ),
        );
        let from_flag = check(&request(&[other_name], &fixture, Some("5.4.0"), false)).unwrap();
        assert_eq!(from_flag.released, 1, "{}", from_flag.stdout);
        assert!(from_flag.stdout.contains("blob=same"));
        assert!(from_flag.stdout.contains("version=5.4.0"));
    }

    #[test]
    fn missing_version_names_the_flag_and_empty_files_need_no_repo() {
        let fixture = fixture();
        let unnamed = write_stack(
            &fixture.dir,
            "rebuild.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n",
                c1 = fixture.c1,
            ),
        );
        let error = check(&request(&[unnamed], &fixture, None, false)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("--easybuild-version"), "{message}");
        assert!(message.contains("eb-X.Y.Z"), "{message}");

        let bad = check(&request(
            &[write_stack(
                &fixture.dir,
                "20261011-eb-5.4.0-UCX.yml",
                "easyconfigs: []\n",
            )],
            &fixture,
            Some("5.4"),
            false,
        ))
        .unwrap_err();
        assert!(format!("{bad:#}").contains("--easybuild-version wants X.Y.Z, got 5.4"));

        let plain = write_stack(
            &fixture.dir,
            "plain.yml",
            "easyconfigs:\n  - zlib-1.2.13.eb\n",
        );
        let report = check(&FromCommitRequest {
            easystacks: vec![plain],
            easyconfigs_repo: None,
            easyblocks_repo: None,
            easybuild_version: None,
            json: false,
            rewrite: false,
        })
        .unwrap();
        assert_eq!(report.stdout, "pins=0 released=0 newer=0 unknown=0\n");

        let pinned = write_stack(
            &fixture.dir,
            "20261011-eb-5.4.0-UCX.yml",
            &format!(
                "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {}\n",
                fixture.c1
            ),
        );
        let missing_repo = check(&FromCommitRequest {
            easystacks: vec![pinned],
            easyconfigs_repo: None,
            easyblocks_repo: None,
            easybuild_version: None,
            json: false,
            rewrite: false,
        })
        .unwrap_err();
        assert!(format!("{missing_repo:#}").contains("--easyconfigs-repo"));
    }

    #[test]
    fn rewrite_drops_same_pins_and_keeps_differing_and_newer_ones() {
        let fixture = fixture();
        // Foo at c1 is in v5.4.0 unchanged (same); Bar at c1 was patched
        // before the tag (differs); New at c3 is after the tag (newer).
        let stack = write_stack(
            &fixture.dir,
            "20261011-eb-5.4.0-rewrite.yml",
            &format!(
                "# header stays\neasyconfigs:\n  - zlib-1.2.13.eb\n  - Foo-1.0.eb:\n      options:\n        # see the pull request\n        from-commit: {c1}\n  - Bar-1.0.eb:\n      options:\n        # same easyconfig as the current installations\n        from-commit: {c1}\n  - New-1.0.eb:\n      options:\n        from-commit: {c3}\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n        include-easyblocks-from-commit: {b0}\n",
                c1 = fixture.c1,
                c3 = fixture.c3,
                b0 = fixture.b0,
            ),
        );
        let mut req = request(std::slice::from_ref(&stack), &fixture, None, false);
        req.rewrite = true;
        let report = check(&req).unwrap();
        assert_eq!(report.rewritten, 2, "{}", report.stdout);
        let text = std::fs::read_to_string(&stack).unwrap();
        assert_eq!(
            text,
            format!(
                "# header stays\neasyconfigs:\n  - zlib-1.2.13.eb\n  - Foo-1.0.eb\n  - Bar-1.0.eb:\n      options:\n        # same easyconfig as the current installations\n        from-commit: {c1}\n  - New-1.0.eb:\n      options:\n        from-commit: {c3}\n  - Foo-1.0.eb:\n      options:\n        include-easyblocks-from-commit: {b0}\n",
                c1 = fixture.c1,
                c3 = fixture.c3,
                b0 = fixture.b0,
            )
        );
        // The differing pins stay released: Bar's easyconfig and the
        // easyblocks tree at b0, which v5.4.0 changed.
        assert_eq!(report.released, 2, "{}", report.stdout);
        assert!(
            report.stdout.ends_with("rewritten=2\n"),
            "{}",
            report.stdout
        );
        // A second run finds nothing left to drop.
        let again = check(&req).unwrap();
        assert_eq!(again.rewritten, 0);
        assert_eq!(std::fs::read_to_string(&stack).unwrap(), text);
    }
}
