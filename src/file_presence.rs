//! Patch and source files an easystack's easyconfigs need, found before a bot build.
//!
//! EasyBuild downloads what a URL names and looks everything else up on
//! disk: patches, sources no URL fetches, and patch files named only in
//! `checksums`. A file that is in neither the easyconfig's directory nor the
//! robot path fails the build on every target, after the job has queued.
//! Software-layer pull request 1842 pinned a commit whose tree lacked pip's
//! patch and failed on all of them.
//!
//! The lookup is EasyBuild's `obtain_file`: the easyconfig's directory, then
//! each robot path, each tried as `<letter>/<name>/`, `<name>/`, and itself,
//! with `extensions/` and `packages/` also tried for an extension's file.
//! A `from-commit` pin puts that commit's whole `easybuild/easyconfigs`
//! tree on the robot path. A Bundle component's patch is fetched twice, once
//! under the bundle's name and once under the component's; for a
//! `from-commit` easyconfig the second lookup skips the easyconfig's
//! directory (easybuild-framework issue 5133).
//!
//! A missing patch or easyconfig fails the check. A source no URL names is
//! a warning: EasyBuild's last resort is sources.easybuild.io, which mirrors
//! the sources of released easyconfigs but not of licensed software.

use crate::cli_parse::usage;
use crate::easystack::{parse_easystack_written, EasystackEntry};
use crate::eb_parse::{resolve_local_files, LocalFile, LocalFileKind, LocalFileOwner};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Arguments for `stack files`.
#[derive(Debug, Clone, Default)]
pub struct FilePresenceRequest {
    /// Easystacks whose entries are checked. Repeatable.
    pub easystacks: Vec<PathBuf>,
    /// Robot-path easyconfig trees, the EasyBuild release the bot runs.
    pub easyconfigs: Vec<PathBuf>,
    /// `COMMIT=DIR`: the easyconfigs tree of one `from-commit` pin.
    pub commit_trees: Vec<String>,
    /// easybuild-easyconfigs git checkout a `from-commit` pin is read from
    /// when no `--commit-tree` names it.
    pub easyconfigs_repo: Option<PathBuf>,
    /// Source directories EasyBuild also searches. Repeatable.
    pub sourcepaths: Vec<PathBuf>,
    /// `FROM=TO`: read an easystack path that starts with `FROM` from `TO`.
    pub path_maps: Vec<String>,
}

/// What a missing file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingKind {
    /// The easyconfig itself is on no robot path.
    Easyconfig,
    /// A `patches` entry of the recipe, an extension, or a component.
    Patch,
    /// A `sources` entry no URL downloads. Reported in
    /// [`FilePresenceReport::no_url`].
    Source,
    /// A patch named only as a `checksums` key.
    ChecksumPatch,
}

impl From<LocalFileKind> for MissingKind {
    fn from(kind: LocalFileKind) -> Self {
        match kind {
            LocalFileKind::Patch => MissingKind::Patch,
            LocalFileKind::Source => MissingKind::Source,
            LocalFileKind::ChecksumPatch => MissingKind::ChecksumPatch,
        }
    }
}

/// One file the bot would not find.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingFile {
    /// Easystack the entry is in.
    pub easystack: String,
    /// Easyconfig filename.
    pub easyconfig: String,
    /// `from-commit` of the entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_commit: Option<String>,
    /// File name EasyBuild asks for.
    pub file: String,
    /// What the file is.
    pub kind: MissingKind,
    /// Recipe, extension, or component that names the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<LocalFileOwner>,
    /// The line the command prints.
    pub message: String,
}

/// What `stack files` prints.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FilePresenceReport {
    /// Stdout, ending with the count line.
    pub stdout: String,
    /// Patches and easyconfigs the bot would not find. Non-empty fails the
    /// command.
    pub missing: Vec<MissingFile>,
    /// Sources no URL names and no tree holds. The bot's last resort is
    /// sources.easybuild.io, which mirrors released sources, so these warn.
    pub no_url: Vec<MissingFile>,
    /// Entries that could not be checked, and why.
    pub unchecked: Vec<String>,
    /// Easyconfigs read.
    pub checked: usize,
    /// Local files looked up across those easyconfigs.
    pub files: usize,
}

enum TreeKind {
    Dir(PathBuf),
    Git {
        repo: PathBuf,
        commit: String,
        files: BTreeSet<String>,
    },
}

/// A directory, or `easybuild/easyconfigs` at one commit of a git checkout.
/// Paths inside it are `/`-separated and relative to its root.
struct Tree {
    kind: TreeKind,
    by_basename: OnceCell<BTreeMap<String, String>>,
}

impl Tree {
    fn dir(root: &Path) -> Self {
        let nested = root.join("easybuild").join("easyconfigs");
        let root = if nested.is_dir() {
            nested
        } else {
            root.to_path_buf()
        };
        Self {
            kind: TreeKind::Dir(root),
            by_basename: OnceCell::new(),
        }
    }

    fn git(repo: &Path, pin: &str) -> Result<Option<Self>> {
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
                "--name-only",
                &commit,
                "--",
                "easybuild/easyconfigs/",
            ],
        )?
        .unwrap_or_default();
        let files = listing
            .lines()
            .filter_map(|line| line.strip_prefix("easybuild/easyconfigs/"))
            .map(str::to_string)
            .collect();
        Ok(Some(Self {
            kind: TreeKind::Git {
                repo: repo.to_path_buf(),
                commit,
                files,
            },
            by_basename: OnceCell::new(),
        }))
    }

    fn has(&self, rel: &str) -> bool {
        match &self.kind {
            TreeKind::Dir(root) => root.join(rel).is_file(),
            TreeKind::Git { files, .. } => files.contains(rel),
        }
    }

    fn read(&self, rel: &str) -> Result<String> {
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

    /// Where EasyBuild's robot finds `file`: `<letter>/<name>/file`, then
    /// anywhere in the tree.
    fn find_easyconfig(&self, file: &str) -> Option<String> {
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
            TreeKind::Git { files, .. } => files.iter().cloned().for_each(&mut add),
            TreeKind::Dir(root) => {
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
                            stack.push((child.path(), rel));
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

/// Easyblocks that derive from PythonPackage, which sets `source_urls` to
/// PyPI when a recipe leaves it unset. From easybuild-easyblocks 5.x.
const PYPI_DEFAULT_EASYBLOCKS: &[&str] = &[
    "CMakePythonPackage",
    "CargoPythonPackage",
    "ConfigureMakePythonPackage",
    "FortranPythonPackage",
    "PythonPackage",
    "VSCPythonPackage",
    "VersionIndependentPythonPackage",
    "EB_EasyBuildMeta",
    "EB_Metagenome_Atlas",
    "EB_PyTorch",
    "EB_PyZMQ",
    "EB_SEPP",
    "EB_TensorFlow",
    "EB_TensorRT",
    "EB_cryptography",
    "EB_dm_minus_reverb",
    "EB_jaxlib",
    "EB_libxml2",
    "EB_netcdf4_minus_python",
    "EB_nose",
    "EB_numexpr",
    "EB_numpy",
    "EB_pybind11",
    "EB_scipy",
    "EB_sympy",
    "EB_tensorflow_minus_compression",
    "EB_torchvision",
    "EB_wxPython",
];

/// The easyblock supplies download URLs for a recipe that names none.
fn easyblock_defaults_urls(easyblock: Option<&str>, name: &str) -> bool {
    let class = match easyblock {
        Some(class) => class.to_string(),
        None => format!(
            "EB_{}",
            name.replace('-', "_minus_")
                .replace('+', "_plus_")
                .replace('.', "_period_")
        ),
    };
    PYPI_DEFAULT_EASYBLOCKS.contains(&class.as_str())
}

fn git_output(repo: &Path, args: &[&str]) -> Result<Option<String>> {
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

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", prefix.trim_end_matches('/'))
    }
}

/// EasyBuild's `letter_dir_for`: the lowercased first letter, `0` otherwise.
fn letter_dir(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_lowercase().to_string(),
        _ => "0".to_string(),
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Where an easyconfig was read from.
struct Located<'t> {
    tree: &'t Tree,
    /// Directory of the easyconfig inside `tree`.
    dir: String,
    /// The same directory as the report names it.
    dir_label: String,
    /// `tree` is one of the robot trees, so the robot-path lookup already
    /// covers the easyconfig's directory.
    on_robot: bool,
}

/// One lookup EasyBuild makes for a file.
struct Lookup<'a> {
    location: &'a str,
    beside: bool,
    extension: bool,
}

fn found(file: &str, lookup: &Lookup<'_>, at: &Located<'_>, roots: &[&Tree]) -> bool {
    let mut bases: Vec<(&Tree, &str)> = Vec::new();
    if lookup.beside {
        bases.push((at.tree, at.dir.as_str()));
    }
    bases.extend(roots.iter().map(|tree| (*tree, "")));
    let letter = letter_dir(lookup.location);
    let subdirs: &[&str] = if lookup.extension {
        &["extensions", "packages", ""]
    } else {
        &[""]
    };
    bases.iter().any(|(tree, base)| {
        let candidates = [
            join(&join(base, &letter), lookup.location),
            join(base, lookup.location),
            base.to_string(),
        ];
        candidates.iter().any(|dir| {
            subdirs
                .iter()
                .any(|sub| tree.has(&join(&join(dir, sub), file)))
        })
    })
}

struct Sources<'t> {
    robot: Vec<Tree>,
    commits: BTreeMap<String, &'t Tree>,
    sourcepaths: Vec<Tree>,
}

/// Check every local file of every easystack entry.
pub fn run_file_presence(request: &FilePresenceRequest) -> Result<FilePresenceReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    let path_maps = request
        .path_maps
        .iter()
        .map(|spec| match spec.split_once('=') {
            Some((from, to)) if !from.is_empty() && !to.is_empty() => {
                Ok((from.to_string(), to.to_string()))
            }
            _ => Err(usage(format!("--path-map wants FROM=TO, got {spec}"))),
        })
        .collect::<Result<Vec<_>>>()?;
    let mut given_commits: Vec<(String, Tree)> = Vec::new();
    for spec in &request.commit_trees {
        let (commit, dir) = spec
            .split_once('=')
            .filter(|(c, d)| !c.is_empty() && !d.is_empty())
            .ok_or_else(|| usage(format!("--commit-tree wants <commit>=<dir>, got {spec}")))?;
        let dir = Path::new(dir);
        if !dir.is_dir() {
            bail!("--commit-tree {spec}: {} is not a directory", dir.display());
        }
        given_commits.push((commit.to_string(), Tree::dir(dir)));
    }
    for path in &request.easyconfigs {
        if !path.is_dir() {
            bail!("--easyconfigs {} is not a directory", path.display());
        }
    }
    if let Some(repo) = &request.easyconfigs_repo {
        if git_output(repo, &["rev-parse", "--git-dir"])?.is_none() {
            bail!(
                "--easyconfigs-repo {} is not a git checkout",
                repo.display()
            );
        }
    }

    let mut parsed: Vec<(String, Vec<(String, EasystackEntry)>)> = Vec::new();
    let mut pins: BTreeSet<String> = BTreeSet::new();
    for path in &request.easystacks {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--easystack {}", path.display()))?;
        let entries = parse_easystack_written(&text)
            .with_context(|| format!("--easystack {}", path.display()))?;
        pins.extend(entries.iter().filter_map(|(_, e)| e.from_commit.clone()));
        parsed.push((path.display().to_string(), entries));
    }

    let mut repo_commits: Vec<(String, Tree)> = Vec::new();
    if let Some(repo) = &request.easyconfigs_repo {
        for pin in &pins {
            if given_commits.iter().any(|(c, _)| same_commit(c, pin)) {
                continue;
            }
            if let Some(tree) = Tree::git(repo, pin)? {
                repo_commits.push((pin.clone(), tree));
            }
        }
    }
    let mut commits: BTreeMap<String, &Tree> = BTreeMap::new();
    for pin in &pins {
        let tree = given_commits
            .iter()
            .chain(repo_commits.iter())
            .find(|(c, _)| same_commit(c, pin))
            .map(|(_, tree)| tree);
        if let Some(tree) = tree {
            commits.insert(pin.clone(), tree);
        }
    }
    let sources = Sources {
        robot: request.easyconfigs.iter().map(|p| Tree::dir(p)).collect(),
        commits,
        sourcepaths: request.sourcepaths.iter().map(|p| Tree::dir(p)).collect(),
    };

    let mut report = FilePresenceReport::default();
    let mut unchecked_lines = Vec::new();
    let mut seen: BTreeSet<(String, String, Option<String>)> = BTreeSet::new();
    for (easystack, entries) in &parsed {
        let stack_pins: BTreeSet<&String> = entries
            .iter()
            .filter_map(|(_, e)| e.from_commit.as_ref())
            .collect();
        let mut roots: Vec<&Tree> = sources.robot.iter().collect();
        roots.extend(
            stack_pins
                .iter()
                .filter_map(|pin| sources.commits.get(*pin).copied()),
        );
        roots.extend(sources.sourcepaths.iter());
        for (written, entry) in entries {
            if !seen.insert((
                easystack.clone(),
                written.clone(),
                entry.from_commit.clone(),
            )) {
                continue;
            }
            check_entry(
                &EntryContext {
                    easystack,
                    written,
                    entry,
                    sources: &sources,
                    roots: &roots,
                    path_maps: &path_maps,
                    sourcepath_given: !request.sourcepaths.is_empty(),
                },
                &mut report,
                &mut unchecked_lines,
            );
        }
    }
    report.unchecked = unchecked_lines;
    let mut stdout = String::new();
    for line in report
        .missing
        .iter()
        .chain(report.no_url.iter())
        .map(|m| m.message.as_str())
        .chain(report.unchecked.iter().map(String::as_str))
    {
        stdout.push_str(line);
        stdout.push('\n');
    }
    stdout.push_str(&format!(
        "missing={} no_url={} unchecked={} easyconfigs={} files={}\n",
        report.missing.len(),
        report.no_url.len(),
        report.unchecked.len(),
        report.checked,
        report.files
    ));
    report.stdout = stdout;
    Ok(report)
}

fn same_commit(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

struct EntryContext<'a> {
    easystack: &'a str,
    written: &'a str,
    entry: &'a EasystackEntry,
    sources: &'a Sources<'a>,
    roots: &'a [&'a Tree],
    path_maps: &'a [(String, String)],
    sourcepath_given: bool,
}

fn check_entry(
    ctx: &EntryContext<'_>,
    report: &mut FilePresenceReport,
    unchecked: &mut Vec<String>,
) {
    let entry = ctx.entry;
    let head = match &entry.from_commit {
        Some(pin) => format!("{} (from-commit {})", entry.file, short(pin)),
        None => entry.file.clone(),
    };
    let written_tree;
    let commit_tree = entry
        .from_commit
        .as_ref()
        .and_then(|pin| ctx.sources.commits.get(pin).copied());
    let located: Located<'_> = if ctx.written.contains('/') {
        let mapped = ctx
            .path_maps
            .iter()
            .find_map(|(from, to)| {
                ctx.written
                    .strip_prefix(from.as_str())
                    .map(|rest| format!("{to}{rest}"))
            })
            .unwrap_or_else(|| ctx.written.to_string());
        let path = PathBuf::from(&mapped);
        let Some(parent) = path.parent().filter(|_| path.is_file()) else {
            unchecked.push(format!(
                "{head}: unchecked, {mapped} is not readable here; --path-map maps its prefix to a local copy"
            ));
            return;
        };
        written_tree = Tree {
            kind: TreeKind::Dir(parent.to_path_buf()),
            by_basename: OnceCell::new(),
        };
        let label = ctx
            .written
            .rsplit_once('/')
            .map(|(dir, _)| format!("{dir}/"))
            .unwrap_or_default();
        Located {
            tree: &written_tree,
            dir: String::new(),
            dir_label: label,
            on_robot: false,
        }
    } else {
        let in_commit =
            commit_tree.and_then(|tree| tree.find_easyconfig(&entry.file).map(|rel| (tree, rel)));
        if let (Some(pin), None) = (&entry.from_commit, commit_tree) {
            unchecked.push(format!(
                "{head}: unchecked, no --commit-tree or --easyconfigs-repo has commit {pin}"
            ));
            return;
        }
        let on_robot = || {
            ctx.sources
                .robot
                .iter()
                .rev()
                .find_map(|tree| tree.find_easyconfig(&entry.file).map(|rel| (tree, rel)))
        };
        let from_commit_tree = in_commit.is_some();
        let Some((tree, rel)) = in_commit.or_else(on_robot) else {
            if ctx.sources.robot.is_empty() && entry.from_commit.is_none() {
                unchecked.push(format!(
                    "{head}: unchecked, pass --easyconfigs for the robot path"
                ));
                return;
            }
            let message = match &entry.from_commit {
                Some(pin) => format!(
                    "{head}: easyconfig is not in commit {} or on the robot path",
                    short(pin)
                ),
                None => format!("{head}: easyconfig is not on the robot path"),
            };
            report.missing.push(MissingFile {
                easystack: ctx.easystack.to_string(),
                easyconfig: entry.file.clone(),
                from_commit: entry.from_commit.clone(),
                file: entry.file.clone(),
                kind: MissingKind::Easyconfig,
                owner: None,
                message,
            });
            return;
        };
        let dir = rel
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        let dir_label = match (&entry.from_commit, from_commit_tree) {
            (Some(pin), true) => format!("{dir}/ at {}", short(pin)),
            _ => format!("{dir}/"),
        };
        Located {
            tree,
            dir_label,
            dir,
            on_robot: !from_commit_tree,
        }
    };

    let file_name = ctx.written.rsplit('/').next().unwrap_or(ctx.written);
    let text = match located.tree.read(&join(&located.dir, file_name)) {
        Ok(text) => text,
        Err(error) => {
            unchecked.push(format!("{head}: unchecked, {error:#}"));
            return;
        }
    };
    let (recipe, files) = match resolve_local_files(&text) {
        Ok(parsed) => parsed,
        Err(error) => {
            unchecked.push(format!("{head}: unchecked, {error}"));
            return;
        }
    };
    report.checked += 1;
    let urls_from_easyblock = easyblock_defaults_urls(recipe.easyblock.as_deref(), &recipe.name);
    for file in files {
        if file.kind == LocalFileKind::Source && urls_from_easyblock {
            continue;
        }
        // `obtain_file` tries the recipe's `source_urls` for a patch too.
        // OpenBLAS lists LAPACK's large.tgz as a patch and netlib serves it;
        // a `.patch` file a pull request adds is on no download site.
        let is_patch_file = file.filename.ends_with(".patch") || file.filename.ends_with(".diff");
        if file.kind == LocalFileKind::Patch && !is_patch_file && !recipe.source_urls.is_empty() {
            continue;
        }
        report.files += 1;
        if file.filename.contains("%(") {
            unchecked.push(format!(
                "{head}: unchecked, {} keeps a template eb-stack does not resolve",
                file.filename
            ));
            continue;
        }
        if let Some(reason) = missing_reason(ctx, &recipe.name, &file, &located) {
            let list = if file.kind == LocalFileKind::Source {
                &mut report.no_url
            } else {
                &mut report.missing
            };
            list.push(MissingFile {
                easystack: ctx.easystack.to_string(),
                easyconfig: entry.file.clone(),
                from_commit: entry.from_commit.clone(),
                file: file.filename.clone(),
                kind: file.kind.into(),
                owner: Some(file.owner.clone()),
                message: format!("{head}: {reason}"),
            });
        }
    }
}

fn describe(file: &LocalFile) -> String {
    let what = match file.kind {
        LocalFileKind::Patch => "patch",
        LocalFileKind::Source => "source",
        LocalFileKind::ChecksumPatch => "checksummed patch",
    };
    let owner = match &file.owner {
        LocalFileOwner::Recipe => String::new(),
        LocalFileOwner::Extension { name, version } => format!(" (extension {name} {version})"),
        LocalFileOwner::Component { name, version } => format!(" (component {name} {version})"),
    };
    format!("{what} {}{owner}", file.filename)
}

fn missing_reason(
    ctx: &EntryContext<'_>,
    recipe_name: &str,
    file: &LocalFile,
    at: &Located<'_>,
) -> Option<String> {
    let location = file.alt_location.as_deref().unwrap_or(recipe_name);
    let parent = Lookup {
        location,
        beside: file.alt_location.is_none(),
        extension: matches!(file.owner, LocalFileOwner::Extension { .. }),
    };
    let robot_dir = format!("{}/{location}/", letter_dir(location));
    let sourcepath = if ctx.sourcepath_given {
        ", or in --sourcepath"
    } else {
        ""
    };
    let parent_found = found(&file.filename, &parent, at, ctx.roots);
    if !parent_found {
        let beside = if parent.beside && !(at.on_robot && at.dir_label == robot_dir) {
            format!("beside the easyconfig in {} or ", at.dir_label)
        } else {
            String::new()
        };
        let lead = match file.kind {
            LocalFileKind::Source => format!("{} has no download URL and", describe(file)),
            _ => describe(file),
        };
        let tail = if file.kind == LocalFileKind::Source {
            "; only sources.easybuild.io can still supply it"
        } else {
            ""
        };
        return Some(format!(
            "{lead} is not {beside}under {robot_dir} on the robot path{sourcepath}{tail}"
        ));
    }
    let LocalFileOwner::Component { name, .. } = &file.owner else {
        return None;
    };
    let component_location = file.alt_location.as_deref().unwrap_or(name);
    let from_commit = ctx.entry.from_commit.is_some();
    let component = Lookup {
        location: component_location,
        beside: file.alt_location.is_none() && !from_commit,
        extension: false,
    };
    if found(&file.filename, &component, at, ctx.roots) {
        return None;
    }
    let component_dir = format!("{}/{component_location}/", letter_dir(component_location));
    if from_commit {
        Some(format!(
            "{} is beside the easyconfig in {}, but for a from-commit easyconfig the component install looks only under {component_dir} on the robot path (easybuild-framework#5133)",
            describe(file),
            at.dir_label
        ))
    } else {
        Some(format!(
            "{} is not under {component_dir} on the robot path, where the component install looks{sourcepath}",
            describe(file)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_options_template_with_the_extension_name() {
        let src = "name = 'Python'\nversion = '3.10.8'\ntoolchain = {'name': 'GCCcore', 'version': '12.2.0'}\n\
source_urls = ['https://www.python.org/ftp/python/%(version)s/']\nsources = [SOURCE_TGZ]\n\
patch_ctypes_ld_library_path = 'Python-3.11.5-custom-ctypes.patch'\n\
checksums = [{'Python-3.10.8.tgz': 'aa'}, {'Python-3.11.5-custom-ctypes.patch': 'bb'}]\n\
exts_list = [\n    ('pip', '22.3.1', {'patches': ['%(name)s-20.3.4_remove-emSign-root-cert.patch']}),\n    ('wheel', '0.38.4'),\n]\n";
        let (recipe, files) = resolve_local_files(src).expect("parse");
        assert_eq!(recipe.name, "Python");
        let names: Vec<(&str, LocalFileKind)> = files
            .iter()
            .map(|f| (f.filename.as_str(), f.kind))
            .collect();
        assert_eq!(
            names,
            vec![
                (
                    "pip-20.3.4_remove-emSign-root-cert.patch",
                    LocalFileKind::Patch
                ),
                (
                    "Python-3.11.5-custom-ctypes.patch",
                    LocalFileKind::ChecksumPatch
                ),
            ]
        );
        assert_eq!(
            files[0].owner,
            LocalFileOwner::Extension {
                name: "pip".into(),
                version: "22.3.1".into()
            }
        );
    }

    #[test]
    fn a_source_without_a_url_is_local_and_a_url_patch_is_not() {
        let src = "name = 'Foo'\nversion = '1.0'\ntoolchain = SYSTEM\n\
sources = ['foo-%(version)s.tar.gz', {'filename': 'bar.tar.gz', 'git_config': {'url': 'x'}}]\n\
patches = ['https://example.org/a.patch', ('b.patch', 1), {'name': 'c.patch', 'alt_location': 'Other'}]\n\
components = [('baz', '2.0', {'patches': ['%(name)s-%(version)s.patch']})]\n";
        let (_, files) = resolve_local_files(src).expect("parse");
        let got: Vec<(&str, LocalFileKind, Option<&str>)> = files
            .iter()
            .map(|f| (f.filename.as_str(), f.kind, f.alt_location.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("b.patch", LocalFileKind::Patch, None),
                ("c.patch", LocalFileKind::Patch, Some("Other")),
                ("foo-1.0.tar.gz", LocalFileKind::Source, None),
                ("baz-2.0.patch", LocalFileKind::Patch, None),
            ]
        );
    }

    #[test]
    fn letter_dir_matches_easybuild() {
        assert_eq!(letter_dir("Python"), "p");
        assert_eq!(letter_dir("xfce4-terminal"), "x");
        assert_eq!(letter_dir("4ti2"), "0");
    }
}
