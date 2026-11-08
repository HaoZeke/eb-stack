//! Easyconfig trees read in place: a directory, or `easybuild/easyconfigs`
//! at one commit of a git object database.
//!
//! A git tree is listed once with `git ls-tree -r`, which needs only tree
//! objects, so a blobless clone serves it. File content is read on demand,
//! one `git show` per file or one `git cat-file --batch` for many; nothing
//! is written to disk.

use anyhow::{bail, Context, Result};
use std::cell::OnceCell;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The directories a walk over a tree on disk has entered, by canonical path.
///
/// A symlink may point back at an ancestor or alias a directory already
/// read; following each one would read the same files again, without end for
/// a cycle and exponentially for nested aliases. A walk enters each real
/// directory once.
#[derive(Default)]
pub(crate) struct DirVisits {
    seen: HashSet<PathBuf>,
}

impl DirVisits {
    /// Whether `dir` has not been entered yet; records it. A directory that
    /// cannot be resolved is not entered.
    pub(crate) fn first(&mut self, dir: &Path) -> bool {
        std::fs::canonicalize(dir).is_ok_and(|real| self.seen.insert(real))
    }
}

/// Where a [`Tree`] reads from.
pub(crate) enum TreeKind {
    /// A directory on disk.
    Dir(PathBuf),
    /// `easybuild/easyconfigs` at `commit` in the object database of `repo`.
    Git {
        repo: PathBuf,
        commit: String,
        /// Path relative to `easybuild/easyconfigs/` to its blob id.
        files: BTreeMap<String, String>,
    },
}

/// A directory, or `easybuild/easyconfigs` at one commit of a git checkout.
/// Paths inside it are `/`-separated and relative to its root.
pub(crate) struct Tree {
    pub(crate) kind: TreeKind,
    by_basename: OnceCell<BTreeMap<String, String>>,
}

impl Tree {
    /// `root`, or `root/easybuild/easyconfigs` when that exists.
    pub(crate) fn dir(root: &Path) -> Self {
        let nested = root.join("easybuild").join("easyconfigs");
        let root = if nested.is_dir() {
            nested
        } else {
            root.to_path_buf()
        };
        Self::at(&root)
    }

    /// Exactly the directory `root`.
    pub(crate) fn at(root: &Path) -> Self {
        Self {
            kind: TreeKind::Dir(root.to_path_buf()),
            by_basename: OnceCell::new(),
        }
    }

    /// `easybuild/easyconfigs` at `pin` in `repo`, or `None` when the
    /// repository has no such commit.
    pub(crate) fn git(repo: &Path, pin: &str) -> Result<Option<Self>> {
        let Some(commit) = git_output(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{pin}^{{commit}}"),
            ],
        )?
        else {
            return Ok(None);
        };
        let listing = git_output(
            repo,
            &[
                "ls-tree",
                "-r",
                "-z",
                &commit,
                "--",
                "easybuild/easyconfigs/",
            ],
        )?
        .unwrap_or_default();
        let mut files = BTreeMap::new();
        for entry in listing.split('\0') {
            let Some((meta, path)) = entry.split_once('\t') else {
                continue;
            };
            let mut parts = meta.split(' ');
            let (_mode, kind, id) = (parts.next(), parts.next(), parts.next());
            if kind != Some("blob") {
                continue;
            }
            let (Some(id), Some(rel)) = (id, path.strip_prefix("easybuild/easyconfigs/")) else {
                continue;
            };
            files.insert(rel.to_string(), id.to_string());
        }
        Ok(Some(Self {
            kind: TreeKind::Git {
                repo: repo.to_path_buf(),
                commit,
                files,
            },
            by_basename: OnceCell::new(),
        }))
    }

    /// The full commit a git tree is at.
    pub(crate) fn commit(&self) -> Option<&str> {
        match &self.kind {
            TreeKind::Git { commit, .. } => Some(commit),
            TreeKind::Dir(_) => None,
        }
    }

    /// Whether the file `rel` exists.
    pub(crate) fn has(&self, rel: &str) -> bool {
        match &self.kind {
            TreeKind::Dir(root) => root.join(rel).is_file(),
            TreeKind::Git { files, .. } => files.contains_key(rel),
        }
    }

    /// The text of `rel`. A git file loses its trailing whitespace.
    pub(crate) fn read(&self, rel: &str) -> Result<String> {
        match &self.kind {
            TreeKind::Dir(root) => {
                let path = root.join(rel);
                std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
            }
            TreeKind::Git { repo, commit, .. } => {
                let spec = format!("{commit}:easybuild/easyconfigs/{rel}");
                git_output(repo, &["show", &spec])?
                    .ok_or_else(|| anyhow::anyhow!("git show {spec} in {}", repo.display()))
            }
        }
    }

    /// The bytes of each of `rels`, exactly as stored. A git tree reads them
    /// through one `git cat-file --batch`.
    pub(crate) fn read_all(&self, rels: &[String]) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut out = BTreeMap::new();
        match &self.kind {
            TreeKind::Dir(root) => {
                for rel in rels {
                    let path = root.join(rel);
                    let bytes =
                        std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                    out.insert(rel.clone(), bytes);
                }
            }
            TreeKind::Git { repo, files, .. } => {
                let mut ids = Vec::with_capacity(rels.len());
                for rel in rels {
                    let id = files
                        .get(rel)
                        .with_context(|| format!("{rel} is not in {}", self.describe()))?;
                    ids.push(id.clone());
                }
                for (rel, bytes) in rels.iter().zip(cat_file_batch(repo, &ids)?) {
                    out.insert(rel.clone(), bytes);
                }
            }
        }
        Ok(out)
    }

    /// Every file under the directory `dir` (`""` for the whole tree), sorted.
    pub(crate) fn files_under(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{}/", dir.trim_end_matches('/'))
        };
        match &self.kind {
            TreeKind::Git { files, .. } => files
                .range(prefix.clone()..)
                .take_while(|(rel, _)| rel.starts_with(&prefix))
                .map(|(rel, _)| rel.clone())
                .collect(),
            TreeKind::Dir(root) => {
                let mut out = Vec::new();
                let mut visits = DirVisits::default();
                let start = root.join(&prefix);
                visits.first(&start);
                let mut stack = vec![(start, prefix.clone())];
                while let Some((path, rel)) = stack.pop() {
                    let Ok(read) = std::fs::read_dir(&path) else {
                        continue;
                    };
                    for child in read.flatten() {
                        let name = child.file_name().to_string_lossy().into_owned();
                        let child_rel = format!("{rel}{name}");
                        let Ok(kind) = child.file_type() else {
                            continue;
                        };
                        if kind.is_dir() || (kind.is_symlink() && child.path().is_dir()) {
                            if !visits.first(&child.path()) {
                                continue;
                            }
                            stack.push((child.path(), format!("{child_rel}/")));
                        } else {
                            out.push(child_rel);
                        }
                    }
                }
                out.sort();
                out
            }
        }
    }

    /// How a candidate parsed from `rel` names its file: the path on disk,
    /// or `<repo>@<commit>:easybuild/easyconfigs/<rel>`.
    pub(crate) fn label(&self, rel: &str) -> String {
        match &self.kind {
            TreeKind::Dir(root) => root.join(rel).display().to_string(),
            TreeKind::Git { repo, commit, .. } => {
                format!("{}@{commit}:easybuild/easyconfigs/{rel}", repo.display())
            }
        }
    }

    fn describe(&self) -> String {
        match &self.kind {
            TreeKind::Dir(root) => root.display().to_string(),
            TreeKind::Git { repo, commit, .. } => format!("{} at {commit}", repo.display()),
        }
    }

    /// Where EasyBuild's robot finds `file`: `<letter>/<name>/file`, then
    /// anywhere in the tree.
    pub(crate) fn find_easyconfig(&self, file: &str) -> Option<String> {
        if let Some(name) = crate::arch_support::easyconfig_name(file) {
            let rel = format!("{}/{name}/{file}", letter_dir(&name));
            if self.has(&rel) {
                return Some(rel);
            }
        }
        self.by_basename
            .get_or_init(|| self.index())
            .get(file)
            .cloned()
    }

    fn index(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut add = |rel: String| {
            if !rel.ends_with(".eb") {
                return;
            }
            let base = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            out.entry(base).or_insert(rel);
        };
        match &self.kind {
            TreeKind::Git { files, .. } => files.keys().cloned().for_each(&mut add),
            TreeKind::Dir(root) => {
                let mut visits = DirVisits::default();
                visits.first(root);
                let mut stack = vec![(root.clone(), String::new())];
                while let Some((dir, prefix)) = stack.pop() {
                    let Ok(read) = std::fs::read_dir(&dir) else {
                        continue;
                    };
                    let mut children: Vec<_> = read.flatten().collect();
                    children.sort_by_key(|entry| entry.file_name());
                    for child in children {
                        let name = child.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            continue;
                        }
                        let rel = join(&prefix, &name);
                        if child.path().is_dir() {
                            if visits.first(&child.path()) {
                                stack.push((child.path(), rel));
                            }
                        } else {
                            add(rel);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Run git in `repo`; `None` when git exits non-zero. Stdout loses its
/// trailing whitespace.
pub(crate) fn git_output(repo: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("run git in {}", repo.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
    ))
}

/// The content of each blob in `ids`, in order, from one
/// `git cat-file --batch`.
fn cat_file_batch(repo: &Path, ids: &[String]) -> Result<Vec<Vec<u8>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run git cat-file in {}", repo.display()))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let request: String = ids.iter().map(|id| format!("{id}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(request.as_bytes()));
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .with_context(|| format!("git cat-file {id} in {}", repo.display()))?;
        let fields: Vec<&str> = header.split_whitespace().collect();
        let size = match fields.as_slice() {
            [_, "blob", size] => size
                .parse::<usize>()
                .with_context(|| format!("git cat-file {id}: {header:?}"))?,
            _ => bail!("git cat-file {id} in {}: {}", repo.display(), header.trim()),
        };
        let mut bytes = vec![0; size];
        reader.read_exact(&mut bytes)?;
        let mut newline = [0u8; 1];
        reader.read_exact(&mut newline)?;
        out.push(bytes);
    }
    drop(reader);
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("git cat-file writer panicked"))?
        .with_context(|| format!("write to git cat-file in {}", repo.display()))?;
    let status = child.wait()?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        bail!("git cat-file in {}: {}", repo.display(), stderr.trim());
    }
    Ok(out)
}

/// `prefix/name`, or `name` when `prefix` is empty.
pub(crate) fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", prefix.trim_end_matches('/'))
    }
}

/// EasyBuild's `letter_dir_for`: the lowercased first letter, `0` otherwise.
pub(crate) fn letter_dir(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_lowercase().to_string(),
        _ => "0".to_string(),
    }
}

/// One lookup EasyBuild makes for a file.
pub(crate) struct Lookup<'a> {
    pub(crate) location: &'a str,
    pub(crate) beside: bool,
    pub(crate) extension: bool,
}

/// Whether `obtain_file` finds `file`: in the easyconfig's directory `dir`
/// of `tree` when `lookup.beside`, then in each of `roots`, each tried as
/// `<letter>/<location>/`, `<location>/`, and itself, with `extensions/` and
/// `packages/` also tried for an extension's file.
pub(crate) fn found_in(
    file: &str,
    lookup: &Lookup<'_>,
    tree: &Tree,
    dir: &str,
    roots: &[&Tree],
) -> bool {
    locate_in(file, lookup, tree, dir, roots).is_some()
}

/// The first place [`found_in`] finds `file`, in the order `obtain_file`
/// tries them: the tree it is in and the path inside that tree.
pub(crate) fn locate_in<'t>(
    file: &str,
    lookup: &Lookup<'_>,
    tree: &'t Tree,
    dir: &str,
    roots: &[&'t Tree],
) -> Option<(&'t Tree, String)> {
    let mut bases: Vec<(&Tree, &str)> = Vec::new();
    if lookup.beside {
        bases.push((tree, dir));
    }
    bases.extend(roots.iter().map(|tree| (*tree, "")));
    let letter = letter_dir(lookup.location);
    let subdirs: &[&str] = if lookup.extension {
        &["extensions", "packages", ""]
    } else {
        &[""]
    };
    bases.into_iter().find_map(|(tree, base)| {
        let candidates = [
            join(&join(base, &letter), lookup.location),
            join(base, lookup.location),
            base.to_string(),
        ];
        candidates.iter().find_map(|dir| {
            subdirs.iter().find_map(|sub| {
                let rel = join(&join(dir, sub), file);
                tree.has(&rel).then_some((tree, rel))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.org")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.org")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_git_tree_lists_blob_ids_and_reads_exact_bytes_in_one_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        git(repo, &["init", "-q"]);
        let a = repo.join("easybuild/easyconfigs/a/A/A-1.0.eb");
        let p = repo.join("easybuild/easyconfigs/a/A/A-1.0.patch");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, "name = 'A'\n\n").unwrap();
        std::fs::write(&p, b"\x00binary\n").unwrap();
        std::fs::write(repo.join("README"), "outside\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "one"]);

        let tree = Tree::git(repo, "HEAD").unwrap().expect("HEAD exists");
        assert_eq!(tree.commit().unwrap().len(), 40);
        assert_eq!(tree.files_under(""), ["a/A/A-1.0.eb", "a/A/A-1.0.patch"]);
        assert_eq!(tree.files_under("a/A"), tree.files_under(""));
        assert!(tree.files_under("a/AB").is_empty());
        assert_eq!(
            tree.find_easyconfig("A-1.0.eb").as_deref(),
            Some("a/A/A-1.0.eb")
        );
        let read = tree.read_all(&tree.files_under("")).unwrap();
        assert_eq!(read["a/A/A-1.0.eb"], b"name = 'A'\n\n");
        assert_eq!(read["a/A/A-1.0.patch"], b"\x00binary\n");
        assert!(tree
            .label("a/A/A-1.0.eb")
            .ends_with(":easybuild/easyconfigs/a/A/A-1.0.eb"));
        assert!(Tree::git(repo, "0000000").unwrap().is_none());
    }

    #[test]
    fn a_directory_tree_lists_and_reads_the_same_way() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("easybuild/easyconfigs");
        std::fs::create_dir_all(root.join("a/A")).unwrap();
        std::fs::write(root.join("a/A/A-1.0.eb"), "x").unwrap();
        let tree = Tree::dir(tmp.path());
        assert_eq!(tree.files_under(""), ["a/A/A-1.0.eb"]);
        assert_eq!(
            tree.read_all(&["a/A/A-1.0.eb".into()]).unwrap()["a/A/A-1.0.eb"],
            b"x"
        );
        assert!(tree.commit().is_none());
    }
}
