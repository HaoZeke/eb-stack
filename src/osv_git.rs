//! Map an OSV GIT range onto release versions.
//!
//! `git ls-remote --tags` names the releases. A commit that is not itself a
//! tag is placed with `git merge-base --is-ancestor` after a blobless fetch
//! of those tags, which keeps the commit graph and skips file contents. The
//! tag list and the fetch are cached. When the lookup cannot run, the caller
//! leaves the range unresolved.

use crate::version::{cmp_version, parse_version_parts, Part};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One release tag: the version a recipe can pin, and the commit it points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTag {
    /// Version parsed from the tag name. A leading `v` is removed.
    pub version: String,
    /// Peeled commit. An annotated tag contributes the commit, not the tag object.
    pub commit: String,
}

/// Where GIT commit bounds are turned into release tags.
///
/// The offline implementation refuses every lookup. The caching implementation
/// shells out to `git` and stores the result.
pub trait GitRangeSource {
    /// Release tags advertised by `repo`, or an error when they are unavailable.
    fn release_tags(&self, repo: &str) -> Result<Vec<ReleaseTag>, String>;
    /// Whether `ancestor` is an ancestor of `descendant`, including equality.
    fn is_ancestor(&self, repo: &str, ancestor: &str, descendant: &str) -> Result<bool, String>;
}

/// GIT lookups that always fail. Resolution then keeps the range unresolved.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoGit;

impl GitRangeSource for NoGit {
    fn release_tags(&self, repo: &str) -> Result<Vec<ReleaseTag>, String> {
        Err(format!("git range lookup is offline ({repo})"))
    }

    fn is_ancestor(&self, repo: &str, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let _ = (ancestor, descendant);
        Err(format!("git range lookup is offline ({repo})"))
    }
}

/// Cached `git ls-remote` and blobless tag fetch.
#[derive(Debug, Clone)]
pub struct CachingGitRanges {
    root: PathBuf,
    enabled: bool,
}

impl CachingGitRanges {
    /// Cache under `EB_STACK_GIT_RANGE_CACHE`, or the user cache directory.
    ///
    /// `EB_STACK_GIT_RANGES=0` turns the lookup off. A disabled lookup fails
    /// the same way an offline machine does, and the range stays unresolved.
    pub fn from_env() -> Self {
        let enabled = !matches!(
            std::env::var("EB_STACK_GIT_RANGES").as_deref(),
            Ok("0") | Ok("false") | Ok("off")
        );
        let root = std::env::var_os("EB_STACK_GIT_RANGE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(default_cache_root);
        Self { root, enabled }
    }
}

impl GitRangeSource for CachingGitRanges {
    fn release_tags(&self, repo: &str) -> Result<Vec<ReleaseTag>, String> {
        if !self.enabled {
            return Err(format!("git range lookup is offline ({repo})"));
        }
        let repo = https_repo(repo)?;
        let path = self.root.join(cache_key(repo)).join("tags.json");
        if let Some(tags) = read_tag_cache(&path) {
            return Ok(tags);
        }
        let text = git_ok(None, &["ls-remote", "--tags", "--", repo])?;
        let tags = parse_ls_remote_tags(&text);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cache {}: {error}", parent.display()))?;
        }
        if let Ok(body) = serde_json::to_vec(&tags) {
            std::fs::write(&path, body)
                .map_err(|error| format!("cache {}: {error}", path.display()))?;
        }
        Ok(tags)
    }

    fn is_ancestor(&self, repo: &str, ancestor: &str, descendant: &str) -> Result<bool, String> {
        if !self.enabled {
            return Err(format!("git range lookup is offline ({repo})"));
        }
        if same_commit(ancestor, descendant) {
            return Ok(true);
        }
        let git_dir = self.ensure_tags(repo)?;
        let out = git(
            Some(&git_dir),
            &["merge-base", "--is-ancestor", ancestor, descendant],
        )?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(format!(
                "git merge-base --is-ancestor {ancestor} {descendant}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        }
    }
}

impl CachingGitRanges {
    /// Blobless fetch of tags. A later call reuses the directory.
    ///
    /// Depth is left unlimited: a shallow history cannot answer whether an
    /// old introduced commit is an ancestor of a release tag. Blobs are
    /// omitted, so the fetch is the commit graph rather than a full checkout.
    fn ensure_tags(&self, repo: &str) -> Result<PathBuf, String> {
        let repo = https_repo(repo)?;
        let root = self.root.join(cache_key(repo));
        let git_dir = root.join("repo.git");
        let marker = root.join("fetched");
        if !git_dir.join("config").is_file() {
            std::fs::create_dir_all(&root)
                .map_err(|error| format!("cache {}: {error}", root.display()))?;
            let path = git_dir.display().to_string();
            git_ok(None, &["init", "--bare", "-b", "main", &path])?;
            git_ok(Some(&git_dir), &["remote", "add", "origin", repo])?;
        }
        if !marker.is_file() {
            git_ok(
                Some(&git_dir),
                &[
                    "-c",
                    "remote.origin.promisor=true",
                    "-c",
                    "remote.origin.partialclonefilter=blob:none",
                    "fetch",
                    "--filter=blob:none",
                    "--no-write-fetch-head",
                    "origin",
                    "+refs/tags/*:refs/tags/*",
                ],
            )?;
            std::fs::write(&marker, b"ok")
                .map_err(|error| format!("cache {}: {error}", marker.display()))?;
        }
        Ok(git_dir)
    }
}

/// Turn ordered GIT events into version requirements.
///
/// `introduced` `0` is the beginning of the repository. A later `fixed` with
/// no new `introduced` reuses the previous introduction, so two fix commits
/// on the same release do not open a second range from version `0`.
pub fn map_git_events(
    repo: &str,
    events: &[serde_json::Value],
    git: &dyn GitRangeSource,
) -> Result<Vec<String>, String> {
    if repo.is_empty() {
        return Err("GIT range has no repository".to_string());
    }
    let tags = git.release_tags(repo)?;
    if tags.is_empty() {
        return Err(format!("{repo} has no release tags"));
    }
    let mut reqs = Vec::new();
    let mut open: Option<String> = None;
    let mut last_intro: Option<String> = None;
    for event in events {
        let Some(object) = event.as_object() else {
            return Err("event not parsed".to_string());
        };
        let Some((key, value)) = object.iter().next() else {
            return Err("event not parsed".to_string());
        };
        let Some(bound) = value.as_str() else {
            return Err(format!("event {key} not parsed"));
        };
        let bound = bound.trim();
        match key.as_str() {
            "introduced" => {
                if let Some(start) = open.take() {
                    reqs.push(format!(">={start}"));
                }
                let version = introduced_version(repo, bound, &tags, git)?;
                open = Some(version.clone());
                last_intro = Some(version);
            }
            "fixed" => {
                let start = open
                    .take()
                    .or_else(|| last_intro.clone())
                    .unwrap_or_else(|| "0".to_string());
                let version = earliest_containing(repo, bound, &tags, git)?;
                reqs.push(format!(">={start},<{version}"));
            }
            "last_affected" => {
                let start = open
                    .take()
                    .or_else(|| last_intro.clone())
                    .unwrap_or_else(|| "0".to_string());
                let version = latest_ancestor_release(repo, bound, &tags, git)?;
                reqs.push(format!(">={start},<={version}"));
            }
            other => return Err(format!("event {other} not parsed")),
        }
    }
    if let Some(start) = open {
        reqs.push(format!(">={start}"));
    }
    if reqs.is_empty() {
        return Err("GIT range produced no requirement".to_string());
    }
    Ok(reqs)
}

/// Release tags from `git ls-remote --tags` output.
///
/// The peeled `^{}` line wins over the tag object, so an annotated tag is the
/// commit a recipe builds.
pub fn parse_ls_remote_tags(text: &str) -> Vec<ReleaseTag> {
    let mut objects: BTreeMap<String, String> = BTreeMap::new();
    let mut peeled: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else {
            continue;
        };
        let Some(git_ref) = parts.next() else {
            continue;
        };
        let Some(name) = git_ref.strip_prefix("refs/tags/") else {
            continue;
        };
        if let Some(tag) = name.strip_suffix("^{}") {
            peeled.insert(tag.to_string(), sha.to_string());
        } else {
            objects.insert(name.to_string(), sha.to_string());
        }
    }
    let mut tags = Vec::new();
    for (name, object) in objects {
        let Some(version) = version_from_tag(&name) else {
            continue;
        };
        let commit = peeled.get(&name).cloned().unwrap_or(object);
        tags.push(ReleaseTag { version, commit });
    }
    tags.sort_by(|left, right| {
        left.version
            .cmp(&right.version)
            .then(left.commit.cmp(&right.commit))
    });
    tags.dedup();
    tags
}

/// Version named by a tag, or `None` when the tag is not a release.
pub fn version_from_tag(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let stripped = match name.strip_prefix(['v', 'V']) {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => name,
    };
    if !stripped.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    match parse_version_parts(stripped).first() {
        Some(Part::Num(_)) => Some(stripped.to_string()),
        _ => None,
    }
}

fn introduced_version(
    repo: &str,
    bound: &str,
    tags: &[ReleaseTag],
    git: &dyn GitRangeSource,
) -> Result<String, String> {
    if bound == "0" {
        return Ok("0".to_string());
    }
    earliest_containing(repo, bound, tags, git)
}

/// Earliest release whose commit contains `bound`.
///
/// A commit that is itself a tag does not need ancestry: no older release
/// contains a later commit. The version is the lowest tag that points at it.
fn earliest_containing(
    repo: &str,
    bound: &str,
    tags: &[ReleaseTag],
    git: &dyn GitRangeSource,
) -> Result<String, String> {
    if let Some(version) = exact_min_version(bound, tags) {
        return Ok(version);
    }
    let mut best: Option<&ReleaseTag> = None;
    for tag in tags {
        if !git.is_ancestor(repo, bound, &tag.commit)? {
            continue;
        }
        best = Some(match best {
            None => tag,
            Some(prev) if version_before(&tag.version, &prev.version) => tag,
            Some(prev) => prev,
        });
    }
    best.map(|tag| tag.version.clone())
        .ok_or_else(|| format!("no release tag contains {bound}"))
}

/// Latest release that is an ancestor of `bound`, counting equality.
fn latest_ancestor_release(
    repo: &str,
    bound: &str,
    tags: &[ReleaseTag],
    git: &dyn GitRangeSource,
) -> Result<String, String> {
    let mut best: Option<&ReleaseTag> = None;
    for tag in tags {
        let contains =
            same_commit(bound, &tag.commit) || git.is_ancestor(repo, &tag.commit, bound)?;
        if !contains {
            continue;
        }
        best = Some(match best {
            None => tag,
            Some(prev) if version_before(&prev.version, &tag.version) => tag,
            Some(prev) => prev,
        });
    }
    best.map(|tag| tag.version.clone())
        .ok_or_else(|| format!("no release tag is an ancestor of {bound}"))
}

/// Lowest version among tags that point at `commit`.
fn exact_min_version(commit: &str, tags: &[ReleaseTag]) -> Option<String> {
    tags.iter()
        .filter(|tag| same_commit(commit, &tag.commit))
        .min_by(|left, right| {
            cmp_version(&left.version, &right.version).then(left.version.cmp(&right.version))
        })
        .map(|tag| tag.version.clone())
}

fn version_before(left: &str, right: &str) -> bool {
    match cmp_version(left, right) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => left < right,
    }
}

fn same_commit(left: &str, right: &str) -> bool {
    !left.is_empty() && left.eq_ignore_ascii_case(right)
}

fn https_repo(repo: &str) -> Result<&str, String> {
    let repo = repo.trim();
    if repo.starts_with("https://")
        && !repo
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
    {
        Ok(repo)
    } else {
        Err(format!("GIT range repository must be https ({repo})"))
    }
}

fn cache_key(repo: &str) -> String {
    repo.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn default_cache_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("eb-stack/git-ranges");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/eb-stack/git-ranges");
    }
    std::env::temp_dir().join("eb-stack-git-ranges")
}

fn read_tag_cache(path: &Path) -> Option<Vec<ReleaseTag>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<Output, String> {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    cmd.output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))
}

fn git_ok(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let out = git(dir, args)?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ls_remote_peels_annotated_tags_and_drops_non_releases() {
        let text = "\
1111111111111111111111111111111111111111\trefs/tags/v1.2.0
2222222222222222222222222222222222222222\trefs/tags/v1.2.0^{}
3333333333333333333333333333333333333333\trefs/tags/1.0.0
4444444444444444444444444444444444444444\trefs/tags/not-a-release
5555555555555555555555555555555555555555\trefs/heads/main
";
        let tags = parse_ls_remote_tags(text);
        assert_eq!(
            tags,
            vec![
                ReleaseTag {
                    version: "1.0.0".into(),
                    commit: "3333333333333333333333333333333333333333".into(),
                },
                ReleaseTag {
                    version: "1.2.0".into(),
                    commit: "2222222222222222222222222222222222222222".into(),
                },
            ]
        );
    }

    #[test]
    fn a_disabled_lookup_does_not_run_git() {
        let git = CachingGitRanges {
            root: std::env::temp_dir().join("eb-stack-git-ranges-disabled"),
            enabled: false,
        };
        let error = git
            .release_tags("https://example.invalid/widget")
            .unwrap_err();
        assert!(error.contains("offline"), "{error}");
    }

    struct LocalTags {
        path: PathBuf,
    }

    impl GitRangeSource for LocalTags {
        fn release_tags(&self, _repo: &str) -> Result<Vec<ReleaseTag>, String> {
            let text = git_ok(
                Some(&self.path),
                &[
                    "for-each-ref",
                    "--format=%(refname:short) %(objectname) %(*objectname)",
                    "refs/tags",
                ],
            )?;
            let mut tags = Vec::new();
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                let Some(name) = parts.next() else {
                    continue;
                };
                let Some(object) = parts.next() else {
                    continue;
                };
                let peeled = parts.next().filter(|value| !value.is_empty());
                let Some(version) = version_from_tag(name) else {
                    continue;
                };
                tags.push(ReleaseTag {
                    version,
                    commit: peeled.unwrap_or(object).to_string(),
                });
            }
            Ok(tags)
        }

        fn is_ancestor(
            &self,
            _repo: &str,
            ancestor: &str,
            descendant: &str,
        ) -> Result<bool, String> {
            if same_commit(ancestor, descendant) {
                return Ok(true);
            }
            let out = git(
                Some(&self.path),
                &["merge-base", "--is-ancestor", ancestor, descendant],
            )?;
            match out.status.code() {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            }
        }
    }

    fn git_repo(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Range Test")
            .env("GIT_AUTHOR_EMAIL", "range@example.com")
            .env("GIT_COMMITTER_NAME", "Range Test")
            .env("GIT_COMMITTER_EMAIL", "range@example.com")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?} {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn rev(dir: &Path) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn merge_base_maps_a_commit_between_tags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git_repo(path, &["init", "-b", "main"]);
        std::fs::write(path.join("note"), "a\n").unwrap();
        git_repo(path, &["add", "note"]);
        git_repo(path, &["commit", "-m", "1.0.0"]);
        let first = rev(path);
        git_repo(path, &["tag", "v1.0.0"]);
        std::fs::write(path.join("note"), "b\n").unwrap();
        git_repo(path, &["add", "note"]);
        git_repo(path, &["commit", "-m", "between"]);
        let between = rev(path);
        std::fs::write(path.join("note"), "c\n").unwrap();
        git_repo(path, &["add", "note"]);
        git_repo(path, &["commit", "-m", "1.1.0"]);
        git_repo(path, &["tag", "v1.1.0"]);
        std::fs::write(path.join("note"), "d\n").unwrap();
        git_repo(path, &["add", "note"]);
        git_repo(path, &["commit", "-m", "1.2.0"]);
        let fixed = rev(path);
        git_repo(path, &["tag", "-a", "v1.2.0", "-m", "release"]);
        let tags = LocalTags {
            path: path.to_path_buf(),
        };
        let introduced = map_git_events(
            "https://example.invalid/widget",
            &[
                serde_json::json!({"introduced": between}),
                serde_json::json!({"fixed": fixed}),
            ],
            &tags,
        )
        .unwrap();
        assert_eq!(introduced, vec![">=1.1.0,<1.2.0".to_string()]);
        let last = map_git_events(
            "https://example.invalid/widget",
            &[
                serde_json::json!({"introduced": first}),
                serde_json::json!({"last_affected": between}),
            ],
            &tags,
        )
        .unwrap();
        assert_eq!(last, vec![">=1.0.0,<=1.0.0".to_string()]);
    }
}
