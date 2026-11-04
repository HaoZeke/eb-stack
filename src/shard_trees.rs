//! Easyconfig trees for build-list lines whose easyconfigs EasyBuild fetches
//! from GitHub.
//!
//! `eb --from-commit=SHA` downloads the easybuild-easyconfigs archive of that
//! commit and puts its `easybuild/easyconfigs` first on the robot path.
//! `eb --from-pr=N` downloads the easyconfig files pull request N changes, at
//! its head, and puts them first. `stack shard` resolves such a line the same
//! way: the line's tree first, then the robot path.
//!
//! A tree is found, in this order, in an explicit `COMMIT=DIR` or `N=DIR`
//! entry, in the cache directory, or by downloading it into the cache. A
//! commit is served from
//! `https://codeload.github.com/<repo>/tar.gz/<sha>`; a commit on a fork
//! that a pull request carries is served through the upstream repository too.
//! Only the `.eb` files under `easybuild/easyconfigs/` are extracted. A pull
//! request's head commit and changed files come from `api.github.com` and the
//! file text from `raw.githubusercontent.com`; its cache entry is keyed by
//! the head commit, so a pull request that moves is fetched again.
//!
//! A tree is written under a temporary name and renamed into place, so a
//! cache entry that exists is complete, and two planners filling one cache at
//! once both end with the same entry.

use crate::pr_overlap::HttpGet;
use crate::shard::is_commit_sha;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// The repository `--from-commit` and `--from-pr` read.
pub const EASYCONFIGS_REPO: &str = "easybuilders/easybuild-easyconfigs";

/// File in a downloaded commit tree listing `sha256  relative/path.eb` for
/// every easyconfig in it, so a planner can skip the files the robot path
/// already has without reading them.
pub const MANIFEST: &str = ".eb-stack-sha256";

/// Where the trees of fetched lines come from.
pub struct TreeSources<'a> {
    /// `--commit-tree COMMIT=DIR`: a commit, full or abbreviated, to an
    /// easyconfigs directory.
    pub commit_trees: BTreeMap<String, PathBuf>,
    /// `--pr-tree N=DIR`: a pull request to a directory of its easyconfigs.
    pub pr_trees: BTreeMap<u64, PathBuf>,
    /// `--commit-cache DIR`: trees found or downloaded there.
    pub cache: Option<PathBuf>,
    /// The client downloads go through.
    pub http: &'a dyn HttpGet,
    /// `owner/name` of the easyconfigs repository.
    pub repo: String,
}

impl TreeSources<'_> {
    /// Whether any source is configured.
    pub fn any(&self) -> bool {
        !self.commit_trees.is_empty() || !self.pr_trees.is_empty() || self.cache.is_some()
    }

    /// The easyconfigs directory of `sha`, or why there is none.
    pub fn commit_tree(&self, sha: &str) -> Result<PathBuf, String> {
        if !is_commit_sha(sha) {
            return Err(format!("{sha} is not a commit sha"));
        }
        let given: Vec<&PathBuf> = self
            .commit_trees
            .iter()
            .filter(|(key, _)| same_commit(key, sha))
            .map(|(_, dir)| dir)
            .collect();
        match given.as_slice() {
            [dir] => return Ok((*dir).clone()),
            [] => {}
            _ => return Err(format!("--commit-tree names {sha} more than once")),
        }
        let Some(cache) = &self.cache else {
            return Err(format!(
                "no easyconfigs for commit {sha}: pass --commit-tree {sha}=DIR or --commit-cache DIR"
            ));
        };
        let entry = cache.join(sha.to_ascii_lowercase());
        let tree = entry.join("easybuild/easyconfigs");
        if tree.is_dir() {
            return Ok(tree);
        }
        let url = format!("https://codeload.github.com/{}/tar.gz/{sha}", self.repo);
        let response = self
            .http
            .get(&url)
            .map_err(|error| format!("commit {sha}: {url}: {error}"))?;
        if response.status != 200 {
            return Err(format!("commit {sha}: {url}: HTTP {}", response.status));
        }
        fill(cache, &entry, |tmp| {
            extract_easyconfigs(&response.body, tmp)
        })
        .map_err(|error| format!("commit {sha}: {url}: {error}"))?;
        Ok(tree)
    }

    /// The directory of pull request `number`'s easyconfigs at its head, or
    /// why there is none.
    pub fn pr_tree(&self, number: u64) -> Result<PathBuf, String> {
        if let Some(dir) = self.pr_trees.get(&number) {
            return Ok(dir.clone());
        }
        let Some(cache) = &self.cache else {
            return Err(format!(
                "no easyconfigs for pull request {number}: pass --pr-tree {number}=DIR or --commit-cache DIR"
            ));
        };
        let (head, files) = crate::pr_overlap::pull_head_easyconfigs(self.http, &self.repo, number)
            .map_err(|error| format!("pull request {number}: {error:#}"))?;
        let entry = cache.join(format!("pr-{number}-{}", head.to_ascii_lowercase()));
        if entry.is_dir() {
            return Ok(entry);
        }
        fill(cache, &entry, |tmp| {
            for (path, text) in &files {
                let dst = tmp.join(safe_relative(path)?);
                std::fs::create_dir_all(dst.parent().expect("a file has a parent"))
                    .map_err(|error| error.to_string())?;
                std::fs::write(&dst, text).map_err(|error| error.to_string())?;
            }
            Ok(files.len())
        })
        .map_err(|error| format!("pull request {number}: {error}"))?;
        Ok(entry)
    }
}

/// Whether a `--commit-tree` key and a build-list sha name one commit: equal,
/// or one an abbreviation of the other.
fn same_commit(key: &str, sha: &str) -> bool {
    let (key, sha) = (key.to_ascii_lowercase(), sha.to_ascii_lowercase());
    key.len() >= 7 && sha.len() >= 7 && (key.starts_with(&sha) || sha.starts_with(&key))
}

/// Write a cache entry through `write` into a temporary directory and rename
/// it to `entry`. An entry another process renamed into place first wins.
fn fill(
    cache: &Path,
    entry: &Path,
    write: impl FnOnce(&Path) -> Result<usize, String>,
) -> Result<(), String> {
    std::fs::create_dir_all(cache).map_err(|error| format!("{}: {error}", cache.display()))?;
    let name = entry
        .file_name()
        .expect("a cache entry has a name")
        .to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp = cache.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));
    let written = write(&tmp);
    let result = match written {
        Ok(0) => Err("no easyconfig files".to_string()),
        Ok(_) => match std::fs::rename(&tmp, entry) {
            Ok(()) => Ok(()),
            Err(_) if entry.is_dir() => Ok(()),
            Err(error) => Err(format!("rename to {}: {error}", entry.display())),
        },
        Err(error) => Err(error),
    };
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

/// A repository path as a relative path with no `..`, root, or prefix.
fn safe_relative(path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("unsafe path in the archive: {path}"));
    }
    Ok(relative.to_path_buf())
}

/// Extract the `.eb` files under `easybuild/easyconfigs/` of a GitHub
/// archive (`<repo>-<sha>/` first) into `dest`, at the same relative paths.
/// Returns how many were written.
pub fn extract_easyconfigs(archive: &[u8], dest: &Path) -> Result<usize, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let mut count = 0;
    let mut manifest = String::new();
    for entry in tar.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let Some((_, inner)) = path.split_once('/') else {
            continue;
        };
        if !inner.starts_with("easybuild/easyconfigs/") || !inner.ends_with(".eb") {
            continue;
        }
        let dst = dest.join(safe_relative(inner)?);
        std::fs::create_dir_all(dst.parent().expect("a file has a parent"))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        std::fs::write(&dst, &bytes).map_err(|error| error.to_string())?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let relative = &inner["easybuild/easyconfigs/".len()..];
        manifest.push_str(&format!("{digest}  {relative}\n"));
        count += 1;
    }
    if count > 0 {
        let tree = dest.join("easybuild/easyconfigs");
        std::fs::write(tree.join(MANIFEST), manifest).map_err(|error| error.to_string())?;
    }
    Ok(count)
}

/// The easyconfigs of `tree` that differ from the robot path's: a file whose
/// name and content match the robot path's effective easyconfig of that name
/// resolves the same either way and is left out. `base` maps a file name to
/// the SHA-256 of the robot path's file. A tree's [`MANIFEST`], when present,
/// stands in for reading its files. Returns the parsed candidates and the
/// files that did not parse.
pub fn overlay_candidates(
    tree: &Path,
    base: &BTreeMap<String, String>,
) -> Result<(Vec<crate::domain::Candidate>, Vec<String>), String> {
    use sha2::{Digest, Sha256};
    let hex = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let same = |relative: &str, digest: &str| {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        base.get(name).is_some_and(|known| known == digest)
    };
    let mut files: Vec<PathBuf> = Vec::new();
    match std::fs::read_to_string(tree.join(MANIFEST)) {
        Ok(manifest) => {
            for line in manifest.lines() {
                let Some((digest, relative)) = line.split_once("  ") else {
                    continue;
                };
                if !same(relative, digest) {
                    files.push(tree.join(safe_relative(relative)?));
                }
            }
        }
        Err(_) => {
            if !tree.is_dir() {
                return Err(format!("{} is not a directory", tree.display()));
            }
            let mut stack = vec![tree.to_path_buf()];
            while let Some(dir) = stack.pop() {
                let entries = std::fs::read_dir(&dir)
                    .map_err(|error| format!("{}: {error}", dir.display()))?;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if entry.file_name() != ".git" {
                            stack.push(path);
                        }
                        continue;
                    }
                    if path.extension().and_then(|e| e.to_str()) != Some("eb") {
                        continue;
                    }
                    let bytes = std::fs::read(&path)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !same(&name, &hex(&bytes)) {
                        files.push(path);
                    }
                }
            }
        }
    }
    files.sort();
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        match crate::eb_parse::parse_easyconfig_file(&path) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => skipped.push(format!("{}: {error}", path.display())),
        }
    }
    Ok((candidates, skipped))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pr_overlap::HttpResponse;
    use std::cell::RefCell;

    /// GET from a map; every URL asked for is recorded.
    #[derive(Default)]
    pub(crate) struct MapHttp {
        pub pages: BTreeMap<String, Vec<u8>>,
        pub asked: RefCell<Vec<String>>,
    }

    impl HttpGet for MapHttp {
        fn get(&self, url: &str) -> Result<HttpResponse, String> {
            self.asked.borrow_mut().push(url.to_string());
            Ok(match self.pages.get(url) {
                Some(body) => HttpResponse {
                    status: 200,
                    body: body.clone(),
                    link: None,
                },
                None => HttpResponse {
                    status: 404,
                    body: b"Not Found".to_vec(),
                    link: None,
                },
            })
        }
    }

    /// A GitHub-shaped `.tar.gz`: every file under `<top>/`.
    pub(crate) fn archive(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{top}/{path}"), text.as_bytes())
                .unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    fn sources<'a>(http: &'a MapHttp, cache: Option<PathBuf>) -> TreeSources<'a> {
        TreeSources {
            commit_trees: BTreeMap::new(),
            pr_trees: BTreeMap::new(),
            cache,
            http,
            repo: EASYCONFIGS_REPO.into(),
        }
    }

    const SHA: &str = "0d6b80c34f3553774e93162cf89088e2423ecf89";

    #[test]
    fn a_commit_is_downloaded_once_and_only_its_easyconfigs_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut http = MapHttp::default();
        http.pages.insert(
            format!("https://codeload.github.com/{EASYCONFIGS_REPO}/tar.gz/{SHA}"),
            archive(
                &format!("easybuild-easyconfigs-{SHA}"),
                &[
                    (
                        "easybuild/easyconfigs/p/petsc4py/petsc4py-3.25.0-foss-2026.1.eb",
                        "name = 'petsc4py'\n",
                    ),
                    (
                        "easybuild/easyconfigs/p/petsc4py/petsc4py-fix.patch",
                        "--- a\n",
                    ),
                    ("test/framework/x.eb", "name = 'x'\n"),
                    ("setup.py", "\n"),
                ],
            ),
        );
        let trees = sources(&http, Some(dir.path().join("cache")));
        let tree = trees.commit_tree(SHA).unwrap();
        assert_eq!(
            tree,
            dir.path()
                .join("cache")
                .join(SHA)
                .join("easybuild/easyconfigs")
        );
        assert!(tree
            .join("p/petsc4py/petsc4py-3.25.0-foss-2026.1.eb")
            .is_file());
        assert!(!tree.join("p/petsc4py/petsc4py-fix.patch").exists());
        assert!(!dir.path().join("cache").join(SHA).join("test").exists());
        let manifest = std::fs::read_to_string(tree.join(MANIFEST)).unwrap();
        assert!(
            manifest.ends_with("  p/petsc4py/petsc4py-3.25.0-foss-2026.1.eb\n"),
            "{manifest}"
        );
        assert_eq!(manifest.lines().count(), 1);
        // The second lookup is the cache: no request.
        assert_eq!(trees.commit_tree(SHA).unwrap(), tree);
        assert_eq!(http.asked.borrow().len(), 1);
        // No temporary directory is left behind.
        let names: Vec<String> = std::fs::read_dir(dir.path().join("cache"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [SHA]);
    }

    #[test]
    fn a_missing_commit_says_why_and_leaves_no_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let http = MapHttp::default();
        let trees = sources(&http, Some(dir.path().join("cache")));
        let why = trees.commit_tree(SHA).unwrap_err();
        assert!(why.contains("HTTP 404"), "{why}");
        assert!(!dir.path().join("cache").join(SHA).exists());
        let none = sources(&http, None).commit_tree(SHA).unwrap_err();
        assert!(none.contains("--commit-cache"), "{none}");
        assert!(sources(&http, None).commit_tree("../etc").is_err());
    }

    #[test]
    fn an_explicit_tree_matches_an_abbreviated_commit() {
        let http = MapHttp::default();
        let mut trees = sources(&http, None);
        trees
            .commit_trees
            .insert(SHA[..12].to_string(), PathBuf::from("/trees/petsc4py"));
        assert_eq!(
            trees.commit_tree(SHA).unwrap(),
            PathBuf::from("/trees/petsc4py")
        );
        assert!(http.asked.borrow().is_empty());
    }

    #[test]
    fn a_pull_request_tree_is_its_changed_easyconfigs_at_its_head() {
        let dir = tempfile::tempdir().unwrap();
        let mut http = MapHttp::default();
        let api = format!("https://api.github.com/repos/{EASYCONFIGS_REPO}/pulls/26954");
        http.pages.insert(
            api.clone(),
            format!(r#"{{"number":26954,"title":"VASP","head":{{"sha":"{SHA}"}}}}"#).into_bytes(),
        );
        http.pages.insert(
            format!("{api}/files?per_page=100&page=1"),
            br#"[{"filename":"easybuild/easyconfigs/v/VASP/VASP-6.6.1-foss-2026.1.eb","status":"added"},
                {"filename":"easybuild/easyconfigs/o/Old/Old-1.eb","status":"removed"},
                {"filename":"README.md","status":"modified"}]"#
                .to_vec(),
        );
        http.pages.insert(
            format!("https://raw.githubusercontent.com/{EASYCONFIGS_REPO}/{SHA}/easybuild/easyconfigs/v/VASP/VASP-6.6.1-foss-2026.1.eb"),
            b"name = 'VASP'\n".to_vec(),
        );
        let trees = sources(&http, Some(dir.path().join("cache")));
        let tree = trees.pr_tree(26954).unwrap();
        assert_eq!(
            tree,
            dir.path().join("cache").join(format!("pr-26954-{SHA}"))
        );
        assert!(tree
            .join("easybuild/easyconfigs/v/VASP/VASP-6.6.1-foss-2026.1.eb")
            .is_file());
        assert!(!tree.join("easybuild/easyconfigs/o").exists());
    }
}
