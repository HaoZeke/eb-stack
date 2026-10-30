//! The newest stable upstream release of an EasyBuild recipe.
//!
//! The backend is read from the recipe's own `source_urls`: PyPI, GitHub,
//! GitLab, CRAN, crates.io, NVIDIA's redistrib manifests, a SourceForge file
//! feed, or a plain HTTP directory. A distribution
//! aggregator such as Repology is not a source of truth here: it orders the
//! versions distributions carry, merges unrelated projects under one name
//! (wget and wget2), and cannot name a package without a per-ecosystem
//! prefix.
//!
//! Candidates pass a staged filter and every rejection keeps its reason:
//! drafts, pre-releases and yanked releases first, then the per-package
//! override (include and exclude patterns, ignored versions, a series policy
//! such as odd minor numbers being development releases, a highest major),
//! then the survivors are sorted with [`cmp_version`] and the first one wins.
//! GitHub's `/releases/latest` is not used: it sorts by date, so a backport
//! can be "latest".

use crate::eb_parse::ResolvedEasyconfig;
use crate::registry::RegistryClient;
use crate::version::cmp_version;
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::Path;

/// The overrides eb-stack ships, keyed by EasyBuild name.
pub const BUILTIN_OVERRIDES: &str = include_str!("../data/upstream-overrides.toml");

/// Where a package's releases are listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Backend {
    /// A PyPI project.
    Pypi {
        /// Project name on PyPI.
        project: String,
    },
    /// A GitHub repository's releases, falling back to its tags.
    Github {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
    },
    /// A GitLab project's releases, falling back to its tags.
    Gitlab {
        /// GitLab host, `gitlab.com`.
        host: String,
        /// Project path, `group/project`.
        path: String,
    },
    /// A CRAN package.
    Cran {
        /// Package name on CRAN.
        package: String,
    },
    /// A crates.io crate.
    Crates {
        /// Crate name.
        name: String,
    },
    /// An HTTP directory listing, scanned for `<prefix><version><suffix>` links.
    Directory {
        /// The listing URL.
        url: String,
        /// File name text before the version.
        prefix: String,
        /// File name text after the version; empty matches any archive
        /// extension.
        #[serde(skip_serializing_if = "String::is_empty")]
        suffix: String,
    },
    /// NVIDIA's `redistrib_<release>.json` manifests under
    /// `developer.download.nvidia.com/compute/<product>/redist/`.
    NvidiaRedist {
        /// `cuda`, `cudnn`, ...
        product: String,
        /// The manifest key whose `version` the recipe carries (`cudnn`);
        /// absent when the release label is the version (the CUDA toolkit).
        #[serde(skip_serializing_if = "Option::is_none")]
        component: Option<String>,
    },
}

impl Backend {
    fn label(&self) -> String {
        match self {
            Self::Pypi { project } => format!("pypi:{project}"),
            Self::Github { owner, repo } => format!("github:{owner}/{repo}"),
            Self::Gitlab { host, path } => format!("gitlab:{host}/{path}"),
            Self::Cran { package } => format!("cran:{package}"),
            Self::Crates { name } => format!("crates:{name}"),
            Self::Directory { url, .. } => format!("directory:{url}"),
            Self::NvidiaRedist { product, .. } => format!("nvidia-redist:{product}"),
        }
    }
}

/// Per-package settings, keyed by EasyBuild name in the overrides TOML.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// `pypi`, `github`, `gitlab`, `cran`, `crates`, `directory`, or `none`
    /// for a recipe that has no upstream release to follow.
    pub backend: Option<String>,
    /// Project for the backend: `owner/repo`, a PyPI or CRAN name, or a URL.
    /// `%(name)s`, `%(namelower)s`, `%(version)s`, `%(version_major)s` and
    /// `%(version_major_minor)s` take the recipe's values.
    pub project: Option<String>,
    /// Only versions matching this pattern are kept.
    pub include: Option<String>,
    /// Versions matching this pattern are dropped.
    pub exclude: Option<String>,
    /// Versions that are never offered.
    #[serde(default)]
    pub ignored: Vec<String>,
    /// `odd-minor-devel`: an odd minor number is a development release.
    pub series_policy: Option<String>,
    /// Versions with a higher first number are a different product line.
    pub max_major: Option<u64>,
    /// Keep exactly this version.
    pub pin: Option<String>,
    /// Why the override exists. Shown in every report.
    pub reason: Option<String>,
}

/// One version a backend listed, and what the filter did with it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Candidate {
    /// The tag or release name as listed.
    pub raw: String,
    /// The version EasyBuild would write.
    pub version: String,
    /// Why it was dropped; absent when it survived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<String>,
}

/// What `package latest` decides for one recipe.
#[derive(Debug, Clone, Serialize)]
pub struct LatestReport {
    /// EasyBuild name.
    pub name: String,
    /// Version the recipe has.
    pub current: String,
    /// Where the versions were read, `pypi:tqdm`.
    pub backend: String,
    /// The newest surviving version; absent when nothing survived.
    pub newest: Option<String>,
    /// `current`, `behind`, `pinned`, or `unknown`.
    pub state: String,
    /// The override's reason, or why the backend could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Every version listed, kept or dropped.
    pub candidates: Vec<Candidate>,
}

/// Parse an overrides TOML document.
pub fn parse_overrides(text: &str) -> Result<BTreeMap<String, Override>> {
    toml::from_str(text).context("parse upstream overrides")
}

/// The shipped overrides, then each file in order; later entries win.
pub fn load_overrides(paths: &[impl AsRef<Path>]) -> Result<BTreeMap<String, Override>> {
    let mut merged = parse_overrides(BUILTIN_OVERRIDES)?;
    for path in paths {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--overrides {}", path.display()))?;
        merged.extend(parse_overrides(&text)?);
    }
    Ok(merged)
}

/// The backend the recipe names, or the one its override sets.
pub fn detect_backend(ec: &ResolvedEasyconfig, ov: Option<&Override>) -> Option<Backend> {
    if let Some(ov) = ov {
        if ov.backend.as_deref() == Some("none") {
            return None;
        }
        if let (Some(kind), Some(project)) = (ov.backend.as_deref(), ov.project.as_deref()) {
            return backend_from_override(kind, project, ec);
        }
    }
    // A `sources` dict may carry its own `source_urls` (ecBuild's GITHUB_SOURCE).
    let source_urls: Vec<&str> = ec
        .source_urls
        .iter()
        .chain(ec.source_files.iter().flat_map(|f| f.source_urls.iter()))
        .map(String::as_str)
        .collect();
    if let Some(backend) = backend_from_urls(&source_urls, ec) {
        return Some(backend);
    }
    if source_urls.is_empty()
        && matches!(
            ec.easyblock.as_deref(),
            Some("PythonPackage" | "CargoPythonPackage")
        )
    {
        // The easyblock adds the PyPI source URL itself; a GitHub homepage
        // does not say where the release is published.
        return Some(Backend::Pypi {
            project: ec.name.clone(),
        });
    }
    if let Some(home) = ec.homepage.as_deref() {
        // A SourceForge homepage stands in for the download only when the
        // recipe names none; otherwise its own directory is read below.
        if let Some(backend) = backend_from_urls(&[home], ec)
            .filter(|b| source_urls.is_empty() || !matches!(b, Backend::Directory { .. }))
        {
            return Some(backend);
        }
        // gnuplot's source_urls is a (url, 'download') pair the parser does
        // not keep; its homepage names the SourceForge project.
        let sf_home =
            Regex::new(r"^https?://([a-z0-9][a-z0-9-]*)\.sourceforge\.(?:net|io)\b").unwrap();
        if source_urls.is_empty() {
            if let Some(c) = sf_home.captures(home) {
                return Some(sourceforge(&c[1], ec));
            }
        }
    }
    let first = source_urls.first()?;
    let (prefix, suffix) = recipe_file_pattern(ec);
    Some(Backend::Directory {
        url: (*first).to_string(),
        prefix,
        suffix,
    })
}

/// The registry backend the first matching URL names.
fn backend_from_urls(urls: &[&str], ec: &ResolvedEasyconfig) -> Option<Backend> {
    let gh = Regex::new(r"^https?://(?:www\.)?github\.com/([^/]+)/([^/#?]+)").unwrap();
    let gl = Regex::new(r"^https?://(gitlab\.[^/]+)/(.+?)/(?:-/|archive|uploads|$)").unwrap();
    // Any GitLab instance serves archives and releases under `/-/`
    // (jugit.fz-juelich.de/mlz/libcerf/-/archive/v3.3/).
    let gl_dash =
        Regex::new(r"^https?://([^/]+)/([^/].*?)/-/(?:archive|releases|tags|raw|package_files)/")
            .unwrap();
    let nvidia = Regex::new(
        r"^https?://developer\.download\.nvidia\.com/compute/([^/]+)/(?:redist/([^/]+)/)?",
    )
    .unwrap();
    let sf = Regex::new(
        r"^https?://(?:(?:download|downloads)\.sourceforge\.net/(?:project/)?|(?:www\.)?sourceforge\.net/projects/)([^/]+)",
    )
    .unwrap();
    for url in urls {
        if url.contains("pypi.python.org")
            || url.contains("files.pythonhosted.org")
            || url.contains("pypi.org/packages")
        {
            let project = url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|p| !p.is_empty() && *p != "source")
                .unwrap_or(&ec.name);
            return Some(Backend::Pypi {
                project: project.to_string(),
            });
        }
        if url.contains("cran.r-project.org") || url.contains("cran.rstudio.com") {
            return Some(Backend::Cran {
                package: ec.name.clone(),
            });
        }
        if url.contains("crates.io") {
            return Some(Backend::Crates {
                name: ec.name.to_lowercase(),
            });
        }
        if let Some(c) = nvidia.captures(url) {
            return Some(Backend::NvidiaRedist {
                product: c[1].to_string(),
                component: c.get(2).map(|m| m.as_str().to_string()),
            });
        }
        if let Some(c) = sf.captures(url) {
            return Some(sourceforge(&c[1], ec));
        }
        if let Some(c) = gh.captures(url) {
            return Some(Backend::Github {
                owner: c[1].to_string(),
                repo: c[2].trim_end_matches(".git").to_string(),
            });
        }
        if let Some(c) = gl.captures(url).or_else(|| gl_dash.captures(url)) {
            return Some(Backend::Gitlab {
                host: c[1].to_string(),
                path: c[2].to_string(),
            });
        }
    }
    None
}

/// A SourceForge project's file feed. It lists the newest files first with
/// their paths, so it reads like a directory listing.
fn sourceforge(project: &str, ec: &ResolvedEasyconfig) -> Backend {
    let (prefix, suffix) = recipe_file_pattern(ec);
    Backend::Directory {
        url: format!("https://sourceforge.net/projects/{project}/rss"),
        prefix,
        suffix,
    }
}

/// The text around the version in the recipe's first source file.
fn recipe_file_pattern(ec: &ResolvedEasyconfig) -> (String, String) {
    let prefix = ec
        .source_files
        .first()
        .map(|f| file_prefix(&f.filename, &ec.version))
        .unwrap_or_else(|| format!("{}-", ec.name));
    let suffix = ec
        .source_files
        .first()
        .and_then(|f| file_suffix(&f.filename, &ec.version))
        .unwrap_or_default();
    (prefix, suffix)
}

/// An override's project with the recipe's name and version templates
/// filled in: `https://www.paraview.org/files/v%(version_major_minor)s/`.
fn expand_project(project: &str, ec: &ResolvedEasyconfig) -> String {
    let parts: Vec<&str> = ec.version.split('.').collect();
    let major_minor = parts.iter().take(2).copied().collect::<Vec<_>>().join(".");
    project
        .replace("%(namelower)s", &ec.name.to_lowercase())
        .replace("%(name)s", &ec.name)
        .replace("%(version_major_minor)s", &major_minor)
        .replace("%(version_major)s", parts[0])
        .replace("%(version)s", &ec.version)
}

fn backend_from_override(kind: &str, project: &str, ec: &ResolvedEasyconfig) -> Option<Backend> {
    let project = expand_project(project, ec);
    Some(match kind {
        "pypi" => Backend::Pypi { project },
        "github" => {
            let (owner, repo) = project.split_once('/')?;
            Backend::Github {
                owner: owner.to_string(),
                repo: repo.to_string(),
            }
        }
        "gitlab" => {
            let (host, path) = project.split_once('/')?;
            Backend::Gitlab {
                host: host.to_string(),
                path: path.to_string(),
            }
        }
        "cran" => Backend::Cran { package: project },
        "crates" => Backend::Crates { name: project },
        "directory" => {
            // The listing moves; the file name is still the recipe's.
            let (prefix, suffix) = recipe_file_pattern(ec);
            Backend::Directory {
                url: project,
                prefix,
                suffix,
            }
        }
        _ => return None,
    })
}

/// The file name text after the version, `-linux-x64.tar.xz` or
/// `+-src.tar.gz`; `None` when the name does not hold the version.
fn file_suffix(filename: &str, version: &str) -> Option<String> {
    let at = filename.find(version)?;
    let rest = &filename[at + version.len()..];
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The part of a source filename before its version: `FastTree-2.2.c` gives `FastTree-`.
fn file_prefix(filename: &str, version: &str) -> String {
    match filename.find(version) {
        Some(at) if at > 0 => filename[..at].to_string(),
        _ => filename
            .split(|c: char| c.is_ascii_digit())
            .next()
            .unwrap_or("")
            .to_string(),
    }
}

/// A version string that names a pre-release, including PEP 440 forms.
pub fn is_prerelease(version: &str) -> bool {
    // A word marker may stand alone ("2.0-beta", "1.0rc"); a single letter
    // needs a number after it ("1.0b1", "3.0a2"), because projects such as
    // libmad (0.15.1b) and tmux (3.6b) use a trailing letter for releases.
    let re = Regex::new(
        r"(?i)(?:\d|[.\-_+])(?:(?:alpha|beta|pre|preview|dev|canary|nightly|snapshot|rc)\d*|[abcm]\d+)(?:[.\-_+]|$)",
    )
    .unwrap();
    re.is_match(version)
}

/// Strip a tag down to the version: `v1.2.3`, `release-1.2.3`, `pkg-1.2.3`.
pub fn tag_version(tag: &str, name: &str) -> String {
    let lower = tag.to_lowercase();
    let lname = name.to_lowercase();
    let mut rest = tag;
    for prefix in [
        format!("{lname}-"),
        format!("{lname}_"),
        lname.clone(),
        "release-".into(),
        "release_".into(),
        "version-".into(),
    ] {
        if lower.starts_with(&prefix) && tag.len() > prefix.len() {
            rest = &tag[prefix.len()..];
            break;
        }
    }
    if rest == tag {
        // A word the name contains, before a separator and a number: libgd
        // tags gd-2.3.3, and hwloc-CUDA reads hwloc's hwloc-2.15.0.
        let squash = |s: &str| -> String {
            s.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase()
        };
        if let Some((word, after)) = tag.split_once(['-', '_']) {
            let numeric = after
                .trim_start_matches(['v', 'V'])
                .starts_with(|c: char| c.is_ascii_digit());
            if word.len() >= 2
                && word.chars().all(|c| c.is_ascii_alphabetic())
                && squash(name).contains(&word.to_lowercase())
                && numeric
            {
                rest = after;
            }
        }
    }
    let rest = rest
        .strip_prefix('v')
        .or_else(|| rest.strip_prefix('V'))
        .unwrap_or(rest);
    rest.to_string()
}

fn get_json(client: &dyn RegistryClient, url: &str) -> Result<serde_json::Value> {
    let body = client.get(url).map_err(|e| anyhow::anyhow!("{e}"))?;
    serde_json::from_slice(&body).with_context(|| format!("parse {url}"))
}

/// Every version the backend lists, before filtering. Already-dropped
/// entries (drafts, pre-release flags, yanked files) carry their reason.
pub fn list_candidates(
    backend: &Backend,
    name: &str,
    client: &dyn RegistryClient,
) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    match backend {
        Backend::Pypi { project } => {
            let doc = get_json(client, &format!("https://pypi.org/pypi/{project}/json"))?;
            if let Some(releases) = doc["releases"].as_object() {
                for (version, files) in releases {
                    let files = files.as_array().cloned().unwrap_or_default();
                    let dropped = if files.is_empty() {
                        Some("no files".to_string())
                    } else if files.iter().all(|f| f["yanked"].as_bool() == Some(true)) {
                        Some("yanked".to_string())
                    } else {
                        None
                    };
                    out.push(Candidate {
                        raw: version.clone(),
                        version: version.clone(),
                        dropped,
                    });
                }
            }
        }
        Backend::Github { owner, repo } => {
            let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=100");
            let releases = get_json(client, &url)?;
            let list = releases.as_array().cloned().unwrap_or_default();
            for r in &list {
                let tag = r["tag_name"].as_str().unwrap_or("").to_string();
                let dropped = if r["draft"].as_bool() == Some(true) {
                    Some("draft".to_string())
                } else if r["prerelease"].as_bool() == Some(true) {
                    Some("marked pre-release".to_string())
                } else {
                    None
                };
                out.push(Candidate {
                    version: tag_version(&tag, name),
                    raw: tag,
                    dropped,
                });
            }
            if list.is_empty() {
                let tags = get_json(
                    client,
                    &format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=100"),
                )?;
                for t in tags.as_array().cloned().unwrap_or_default() {
                    let tag = t["name"].as_str().unwrap_or("").to_string();
                    out.push(Candidate {
                        version: tag_version(&tag, name),
                        raw: tag,
                        dropped: None,
                    });
                }
            }
        }
        Backend::Gitlab { host, path } => {
            let id = path.replace('/', "%2F");
            let releases = get_json(
                client,
                &format!("https://{host}/api/v4/projects/{id}/releases?per_page=100"),
            )?;
            let list = releases.as_array().cloned().unwrap_or_default();
            for r in &list {
                let tag = r["tag_name"].as_str().unwrap_or("").to_string();
                let dropped =
                    (r["upcoming_release"].as_bool() == Some(true)).then(|| "upcoming".to_string());
                out.push(Candidate {
                    version: tag_version(&tag, name),
                    raw: tag,
                    dropped,
                });
            }
            if list.is_empty() {
                let tags = get_json(
                    client,
                    &format!("https://{host}/api/v4/projects/{id}/repository/tags?per_page=100"),
                )?;
                for t in tags.as_array().cloned().unwrap_or_default() {
                    let tag = t["name"].as_str().unwrap_or("").to_string();
                    out.push(Candidate {
                        version: tag_version(&tag, name),
                        raw: tag,
                        dropped: None,
                    });
                }
            }
        }
        Backend::Cran { package } => {
            let url = format!("https://cran.r-project.org/web/packages/{package}/DESCRIPTION");
            let body = client.get(&url).map_err(|e| anyhow::anyhow!("{e}"))?;
            let text = String::from_utf8_lossy(&body);
            if let Some(v) = text.lines().find_map(|l| l.strip_prefix("Version:")) {
                let v = v.trim().to_string();
                out.push(Candidate {
                    raw: v.clone(),
                    version: v,
                    dropped: None,
                });
            }
        }
        Backend::Crates { name } => {
            let doc = get_json(client, &format!("https://crates.io/api/v1/crates/{name}"))?;
            for v in doc["versions"].as_array().cloned().unwrap_or_default() {
                let num = v["num"].as_str().unwrap_or("").to_string();
                let dropped = (v["yanked"].as_bool() == Some(true)).then(|| "yanked".to_string());
                out.push(Candidate {
                    raw: num.clone(),
                    version: num,
                    dropped,
                });
            }
        }
        Backend::NvidiaRedist { product, component } => {
            out = nvidia_candidates(product, component.as_deref(), client)?;
        }
        Backend::Directory {
            url,
            prefix,
            suffix,
        } => {
            // A query string names a download (download.php?version=1.7.7l),
            // so the page without it is the listing, when it can be read.
            let base = url
                .split_once('?')
                .and_then(|(base, _)| client.get(base).ok());
            let mut pages = vec![match base {
                Some(body) => String::from_utf8_lossy(&body).into_owned(),
                None => read_listing(url, client)?,
            }];
            for dir in versioned_dirs(url, client) {
                if dir != *url {
                    if let Ok(page) = read_listing(&dir, client) {
                        pages.push(page);
                    }
                }
            }
            // A dash in the version must open another number, so a platform
            // build (`blender-4.0.2-windows-x64.zip`) is not a version.
            let tail = if suffix.is_empty() {
                r"(?:\.tar\.gz|\.tar\.bz2|\.tar\.xz|\.tgz|\.zip|\.c|\.tar)\b".to_string()
            } else {
                regex::escape(suffix)
            };
            let re = Regex::new(&format!(
                r"{}([0-9][0-9A-Za-z._]*?(?:-[0-9][0-9A-Za-z._]*?)*){tail}",
                regex::escape(prefix)
            ))?;
            let mut seen = std::collections::BTreeSet::new();
            for html in &pages {
                for c in re.captures_iter(html) {
                    let v = c[1].trim_end_matches(['.', '-', '_']);
                    // boost_1_92_0: underscores stand for dots.
                    let v = if v.contains('.') {
                        v.to_string()
                    } else {
                        v.replace('_', ".")
                    };
                    if seen.insert(v.clone()) {
                        out.push(Candidate {
                            raw: v.clone(),
                            version: v,
                            dropped: None,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The page at `url`, or at the nearest of its two parent directories that
/// can be read. A host that serves no index for the release directory
/// (`.../tags/scorep-10.1/`, `.../hwloc/v2.13/downloads/`) often lists the
/// same files one or two levels up.
fn read_listing(url: &str, client: &dyn RegistryClient) -> Result<String> {
    let err = match client.get(url) {
        Ok(body) => return Ok(String::from_utf8_lossy(&body).into_owned()),
        Err(err) => anyhow::anyhow!("{err}"),
    };
    let mut dir = url.trim_end_matches('/');
    for _ in 0..2 {
        let Some((parent, _)) = dir.rsplit_once('/') else {
            break;
        };
        // Never above the host: `https://host` has no path left.
        if !parent
            .split_once("://")
            .is_some_and(|(_, p)| p.contains('/'))
        {
            break;
        }
        if let Ok(body) = client.get(&format!("{parent}/")) {
            return Ok(String::from_utf8_lossy(&body).into_owned());
        }
        dir = parent;
    }
    Err(err)
}

/// The NVIDIA release labels in `redist/`, and the version each names.
fn nvidia_candidates(
    product: &str,
    component: Option<&str>,
    client: &dyn RegistryClient,
) -> Result<Vec<Candidate>> {
    let base = format!("https://developer.download.nvidia.com/compute/{product}/redist/");
    let body = client.get(&base).map_err(|e| anyhow::anyhow!("{e}"))?;
    let html = String::from_utf8_lossy(&body);
    let re = Regex::new(r"redistrib_(\d+(?:\.\d+)+)\.json").unwrap();
    let mut labels: Vec<String> = re.captures_iter(&html).map(|c| c[1].to_string()).collect();
    labels.sort_by(|a, b| cmp_version(b, a));
    labels.dedup();
    let Some(component) = component else {
        // The CUDA toolkit's release label is its version.
        return Ok(labels
            .into_iter()
            .map(|label| Candidate {
                raw: format!("redistrib_{label}.json"),
                version: label,
                dropped: None,
            })
            .collect());
    };
    // A library's own version (cuDNN 9.27.0.42) is longer than the label
    // (9.27.0); only the newest manifest is read for it.
    let Some(label) = labels.first() else {
        return Ok(Vec::new());
    };
    let raw = format!("redistrib_{label}.json");
    let doc = get_json(client, &format!("{base}{raw}"))?;
    let version = doc[component]["version"]
        .as_str()
        .with_context(|| format!("{base}{raw} names no {component} version"))?;
    Ok(vec![Candidate {
        raw,
        version: version.to_string(),
        dropped: None,
    }])
}

/// Why a recipe has no backend.
fn no_backend_reason(ec: &ResolvedEasyconfig) -> &'static str {
    if ec.has_components
        || ec
            .easyblock
            .as_deref()
            .is_some_and(|b| b.contains("Bundle"))
    {
        "a bundle has no upstream release of its own; its components do"
    } else if ec.source_urls.is_empty() {
        "the recipe names no source_urls"
    } else {
        "no backend reads these source_urls"
    }
}

/// Directories beside a versioned path segment, newest first, at most two:
/// `.../release/Blender4.0/` lists `.../release/` and keeps `Blender5.0/`
/// and `Blender4.5/`; `.../hwloc/v2.12/downloads/` keeps the part after the
/// segment. Empty when no segment holds a dotted version or the parent
/// cannot be read.
fn versioned_dirs(url: &str, client: &dyn RegistryClient) -> Vec<String> {
    let Some(c) = Regex::new(r"^(https?://.+/)([^/]*?)(\d+(?:\.\d+)+)([^/]*)/(.*)$")
        .unwrap()
        .captures(url)
    else {
        return Vec::new();
    };
    let (parent, pre, post, tail) = (&c[1], &c[2], &c[4], &c[5]);
    let Ok(body) = client.get(parent) else {
        return Vec::new();
    };
    let html = String::from_utf8_lossy(&body);
    // A page the parent redirects to may link the directories by a longer
    // path (`../../software/hwloc/v2.15/`); that path still ends in the
    // parent's own name.
    let parent_name = parent
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let Ok(re) = Regex::new(&format!(
        r#"href="(?:\./|[^"]*/{}/)?{}(\d+(?:\.\d+)+){}/?""#,
        regex::escape(parent_name),
        regex::escape(pre),
        regex::escape(post)
    )) else {
        return Vec::new();
    };
    let mut versions: Vec<String> = re.captures_iter(&html).map(|v| v[1].to_string()).collect();
    versions.sort_by(|a, b| cmp_version(b, a));
    versions.dedup();
    versions
        .into_iter()
        .take(2)
        .map(|v| format!("{parent}{pre}{v}{post}/{tail}"))
        .collect()
}

fn major(version: &str) -> Option<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())?
        .parse()
        .ok()
}

fn minor(version: &str) -> Option<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .nth(1)?
        .parse()
        .ok()
}

/// Apply the staged filter and pick the newest survivor.
pub fn decide(
    name: &str,
    current: &str,
    backend: &str,
    mut candidates: Vec<Candidate>,
    ov: Option<&Override>,
) -> Result<LatestReport> {
    let include = ov
        .and_then(|o| o.include.as_deref())
        .map(Regex::new)
        .transpose()?;
    let exclude = ov
        .and_then(|o| o.exclude.as_deref())
        .map(Regex::new)
        .transpose()?;
    // A package revision on the tag (NCCL's v2.32.3-1) is not part of a
    // version written without a dash.
    let revision = Regex::new(r"^([0-9][0-9.]*[0-9])-[0-9]+$").unwrap();
    for c in candidates.iter_mut().filter(|c| c.dropped.is_none()) {
        if !current.contains('-') {
            if let Some(base) = revision.captures(&c.version).map(|m| m[1].to_string()) {
                c.version = base;
            }
        }
        let v = c.version.as_str();
        c.dropped = if v.is_empty() || !v.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            Some("not a version".into())
        } else if is_prerelease(v) {
            Some("pre-release".into())
        } else if !current.chars().any(|ch| ch.is_ascii_alphabetic())
            && v.chars().any(|ch| ch.is_ascii_alphabetic())
        {
            // nccl4py-v0.6.0 in NCCL's tags, a dated build tag in flatbuffers'.
            Some("letters where the recipe's version has none".into())
        } else if include.as_ref().is_some_and(|re| !re.is_match(v)) {
            Some("outside the include pattern".into())
        } else if exclude.as_ref().is_some_and(|re| re.is_match(v)) {
            Some("matches the exclude pattern".into())
        } else if ov.is_some_and(|o| o.ignored.iter().any(|i| i == v)) {
            Some("ignored by the override".into())
        } else if ov
            .and_then(|o| o.max_major)
            .zip(major(v))
            .is_some_and(|(max, m)| m > max)
        {
            Some("major above the override's max_major".into())
        } else if ov.and_then(|o| o.series_policy.as_deref()) == Some("odd-minor-devel")
            && minor(v).is_some_and(|m| m % 2 == 1)
        {
            Some("odd minor: development series".into())
        } else {
            None
        };
    }
    let mut survivors: Vec<&Candidate> =
        candidates.iter().filter(|c| c.dropped.is_none()).collect();
    // Equal versions (3.26 and 3.26.0) resolve to the spelling with as many
    // parts as the recipe's version.
    let parts = |v: &str| v.split(['.', '-', '_']).count();
    let want = parts(current);
    survivors.sort_by(|a, b| {
        cmp_version(&b.version, &a.version)
            .then_with(|| (parts(&a.version) != want).cmp(&(parts(&b.version) != want)))
            .then_with(|| a.raw.cmp(&b.raw))
    });
    let newest = survivors.first().map(|c| c.version.clone());
    let reason = ov.and_then(|o| o.reason.clone()).or_else(|| {
        candidates
            .is_empty()
            .then(|| "the backend listed no version of the recipe's source file".to_string())
    });
    let (state, newest) = if let Some(pin) = ov.and_then(|o| o.pin.clone()) {
        ("pinned".to_string(), Some(pin))
    } else {
        let state = match &newest {
            None => "unknown",
            Some(n) if cmp_version(n, current) == Ordering::Greater => "behind",
            Some(_) => "current",
        };
        (state.to_string(), newest)
    };
    candidates.sort_by(|a, b| cmp_version(&b.version, &a.version).then_with(|| a.raw.cmp(&b.raw)));
    Ok(LatestReport {
        name: name.to_string(),
        current: current.to_string(),
        backend: backend.to_string(),
        newest,
        state,
        reason,
        candidates,
    })
}

/// The newest stable upstream release of one parsed recipe.
pub fn latest(
    ec: &ResolvedEasyconfig,
    overrides: &BTreeMap<String, Override>,
    client: &dyn RegistryClient,
) -> Result<LatestReport> {
    let ov = overrides.get(&ec.name);
    let Some(backend) = detect_backend(ec, ov) else {
        let mut report = decide(&ec.name, &ec.version, "none", Vec::new(), ov)?;
        report.reason = Some(
            ov.and_then(|o| o.reason.clone())
                .unwrap_or_else(|| no_backend_reason(ec).into()),
        );
        return Ok(report);
    };
    let candidates = match list_candidates(&backend, &ec.name, client) {
        Ok(c) => c,
        Err(err) => {
            let mut report = decide(&ec.name, &ec.version, &backend.label(), Vec::new(), ov)?;
            report.state = "unknown".into();
            report.reason = Some(format!("{err:#}"));
            return Ok(report);
        }
    };
    decide(&ec.name, &ec.version, &backend.label(), candidates, ov)
}

/// The version `package bump --latest` moves to: `None` when the recipe
/// already has the newest stable release.
///
/// # Errors
///
/// No backend answered, or no stable release survived the filter.
pub fn bump_version(report: &LatestReport) -> Result<Option<String>> {
    match (report.state.as_str(), report.newest.as_deref()) {
        ("unknown", _) | (_, None) => anyhow::bail!(
            "--latest: no upstream version decided for {} ({})",
            report.name,
            report.reason.as_deref().unwrap_or("no candidates survived")
        ),
        (_, Some(newest)) if newest == report.current => Ok(None),
        (_, Some(newest)) => Ok(Some(newest.to_string())),
    }
}

/// Render one report as the text `package latest` prints: one line per
/// decision, then every candidate with what happened to it.
pub fn render_text(report: &LatestReport) -> String {
    let mut out = format!(
        "name={} current={} newest={} state={} backend={}",
        report.name,
        report.current,
        report.newest.as_deref().unwrap_or("-"),
        report.state,
        report.backend
    );
    if let Some(r) = &report.reason {
        out.push_str(&format!(" reason={r:?}"));
    }
    out.push('\n');
    for c in &report.candidates {
        match &c.dropped {
            Some(why) => out.push_str(&format!("  dropped {} ({why})\n", c.version)),
            None => out.push_str(&format!("  kept    {}\n", c.version)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MapClient;

    fn ec(name: &str, version: &str, urls: &[&str]) -> ResolvedEasyconfig {
        let mut text = format!(
            "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\nhomepage = 'https://example.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = {urls:?}\nsources = ['%(name)s-%(version)s.tar.gz']\n"
        );
        text = text.replace('"', "'");
        crate::eb_parse::resolve_easyconfig_str(&text).unwrap()
    }

    fn client(pages: &[(&str, &str)]) -> MapClient {
        MapClient {
            pages: pages
                .iter()
                .map(|(u, b)| (u.to_string(), b.as_bytes().to_vec()))
                .collect(),
        }
    }

    #[test]
    fn directory_orders_numerically_so_2_2_beats_2_1_11() {
        let recipe = ec("FastTree", "2.1.11", &["https://example.org/fasttree/"]);
        let page = r#"<a href="FastTree-2.1.11.tar.gz">x</a> <a href="FastTree-2.2.tar.gz">y</a> <a href="FastTree-2.1.9.tar.gz">z</a>"#;
        let report = latest(
            &recipe,
            &BTreeMap::new(),
            &client(&[("https://example.org/fasttree/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("2.2"));
        assert_eq!(report.state, "behind");
    }

    #[test]
    fn directory_walks_up_a_versioned_segment_and_skips_platform_builds() {
        let recipe = ec(
            "Blender",
            "4.0.1",
            &["https://download.example.org/release/Blender4.0/"],
        );
        let parent = r#"<a href="Blender3.6/">a</a> <a href="Blender4.0/">b</a> <a href="Blender4.5/">c</a> <a href="Blender5.0/">d</a>"#;
        let v40 =
            r#"<a href="Blender-4.0.1.tar.gz">x</a> <a href="Blender-4.0.2-windows-x64.zip">y</a>"#;
        let v50 = r#"<a href="Blender-5.0.1.tar.gz">x</a> <a href="Blender-5.0.1-linux-x64.tar.xz">y</a>"#;
        let report = latest(
            &recipe,
            &BTreeMap::new(),
            &client(&[
                ("https://download.example.org/release/Blender4.0/", v40),
                ("https://download.example.org/release/", parent),
                ("https://download.example.org/release/Blender5.0/", v50),
                ("https://download.example.org/release/Blender4.5/", ""),
            ]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("5.0.1"));
        assert!(
            report
                .candidates
                .iter()
                .all(|c| !c.version.contains("windows")),
            "{:?}",
            report.candidates
        );
    }

    #[test]
    fn a_python_package_without_source_urls_reads_pypi() {
        let text = "easyblock = 'PythonPackage'\nname = 'Markdown'\nversion = '3.10.2'\nhomepage = 'https://example.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsources = [SOURCE_TAR_GZ]\n";
        let recipe = crate::eb_parse::resolve_easyconfig_str(text).unwrap();
        assert_eq!(
            detect_backend(&recipe, None),
            Some(Backend::Pypi {
                project: "Markdown".into()
            })
        );
    }

    #[test]
    fn a_binary_tarball_matches_its_own_suffix_and_underscores_read_as_dots() {
        let text = "easyblock = 'Tarball'\nname = 'BLAST+'\nversion = '2.17.0'\nhomepage = 'https://example.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://ftp.example.org/blast+/']\nsources = ['ncbi-blast-%(version)s+-x64-linux.tar.gz']\n";
        let recipe = crate::eb_parse::resolve_easyconfig_str(text).unwrap();
        let page = r#"<a href="ncbi-blast-2.17.0+-x64-linux.tar.gz">a</a> <a href="ncbi-blast-2.18.0+-x64-linux.tar.gz">b</a> <a href="ncbi-blast-2.18.0+-src.tar.gz">c</a>"#;
        let report = latest(
            &recipe,
            &BTreeMap::new(),
            &client(&[("https://ftp.example.org/blast+/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("2.18.0"));

        let boost = ec("Boost", "1.90.0", &["https://example.org/boost/"]);
        let page = r#"<a href="Boost-1_90_0.tar.gz">a</a> <a href="Boost-1_92_0.tar.gz">b</a>"#;
        let report = latest(
            &boost,
            &BTreeMap::new(),
            &client(&[("https://example.org/boost/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("1.92.0"));
    }

    #[test]
    fn a_tag_with_letters_is_not_a_release_of_a_numeric_version() {
        let candidates = [
            "2.30.4",
            "2.30.7",
            "4py-v0.6.0",
            "25.12.19-2026-02-06-03fffb2",
        ]
        .iter()
        .map(|v| Candidate {
            raw: (*v).into(),
            version: (*v).into(),
            dropped: None,
        })
        .collect();
        let report = decide("NCCL", "2.30.4", "github:NVIDIA/nccl", candidates, None).unwrap();
        assert_eq!(report.newest.as_deref(), Some("2.30.7"));
        let tags = |list: &[&str]| -> Vec<Candidate> {
            list.iter()
                .map(|v| Candidate {
                    raw: (*v).into(),
                    version: (*v).into(),
                    dropped: None,
                })
                .collect()
        };
        let revised = decide("NCCL", "2.30.4", "g", tags(&["2.30.4-1", "2.32.3-1"]), None).unwrap();
        assert_eq!(revised.newest.as_deref(), Some("2.32.3"));
        let petsc = decide(
            "PETSc",
            "3.25.0",
            "d",
            tags(&["3.26", "3.26.0", "3.25.0"]),
            None,
        )
        .unwrap();
        assert_eq!(petsc.newest.as_deref(), Some("3.26.0"));
        let empty = decide("x", "1.0", "directory:u", Vec::new(), None).unwrap();
        assert_eq!(empty.state, "unknown");
        assert!(empty.reason.is_some());
    }

    #[test]
    fn a_bundle_says_why_it_has_no_backend() {
        let text = "easyblock = 'PythonBundle'\nname = 'Jupyter-bundle'\nversion = '20260909'\nhomepage = 'https://example.org'\ndescription = 'x'\ntoolchain = SYSTEM\n";
        let recipe = crate::eb_parse::resolve_easyconfig_str(text).unwrap();
        let report = latest(&recipe, &BTreeMap::new(), &client(&[])).unwrap();
        assert_eq!(report.state, "unknown");
        assert!(
            report.reason.as_deref().unwrap_or("").contains("bundle"),
            "{:?}",
            report.reason
        );
    }

    #[test]
    fn max_major_keeps_wget_off_wget2() {
        let recipe = ec("wget", "1.25.0", &["https://ftp.gnu.org/gnu/wget/"]);
        let page = r#"wget-1.25.0.tar.gz wget-1.24.5.tar.gz wget-2.2.0.tar.gz"#;
        let ovs =
            parse_overrides("[wget]\nmax_major = 1\nreason = \"wget2 is a separate product\"\n")
                .unwrap();
        let report = latest(
            &recipe,
            &ovs,
            &client(&[("https://ftp.gnu.org/gnu/wget/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("1.25.0"));
        assert_eq!(report.state, "current");
        assert!(report.candidates.iter().any(|c| c.version == "2.2.0"
            && c.dropped.as_deref() == Some("major above the override's max_major")));
    }

    #[test]
    fn odd_minor_is_a_development_series() {
        let recipe = ec(
            "GStreamer",
            "1.28.0",
            &["https://gstreamer.freedesktop.org/src/gstreamer/"],
        );
        let mut recipe = recipe;
        recipe.source_files.clear();
        let page = "GStreamer-1.28.8.tar.xz GStreamer-1.29.2.tar.xz GStreamer-1.26.9.tar.xz";
        let ovs = parse_overrides("[GStreamer]\nseries_policy = \"odd-minor-devel\"\n").unwrap();
        let report = latest(
            &recipe,
            &ovs,
            &client(&[("https://gstreamer.freedesktop.org/src/gstreamer/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("1.28.8"));
    }

    #[test]
    fn pypi_drops_prereleases_and_yanked_releases() {
        let recipe = ec(
            "tqdm",
            "4.67.3",
            &["https://pypi.python.org/packages/source/t/tqdm"],
        );
        let doc = r#"{"releases": {"4.67.3": [{"yanked": false}], "4.70.1": [{"yanked": false}], "4.71.0": [{"yanked": true}], "5.0.0rc1": [{"yanked": false}]}}"#;
        let report = latest(
            &recipe,
            &BTreeMap::new(),
            &client(&[("https://pypi.org/pypi/tqdm/json", doc)]),
        )
        .unwrap();
        assert_eq!(report.backend, "pypi:tqdm");
        assert_eq!(report.newest.as_deref(), Some("4.70.1"));
        let why: BTreeMap<_, _> = report
            .candidates
            .iter()
            .map(|c| (c.version.as_str(), c.dropped.clone()))
            .collect();
        assert_eq!(why["4.71.0"].as_deref(), Some("yanked"));
        assert_eq!(why["5.0.0rc1"].as_deref(), Some("pre-release"));
    }

    #[test]
    fn github_skips_drafts_and_prereleases_and_strips_the_tag() {
        let recipe = ec(
            "Kokkos",
            "5.1.1",
            &["https://github.com/kokkos/kokkos/releases/download/5.1.1"],
        );
        let doc = r#"[{"tag_name": "5.2.2", "draft": false, "prerelease": false},
                      {"tag_name": "v5.3.0", "draft": true, "prerelease": false},
                      {"tag_name": "5.3.0-rc1", "draft": false, "prerelease": true},
                      {"tag_name": "4.7.4", "draft": false, "prerelease": false}]"#;
        let report = latest(
            &recipe,
            &BTreeMap::new(),
            &client(&[(
                "https://api.github.com/repos/kokkos/kokkos/releases?per_page=100",
                doc,
            )]),
        )
        .unwrap();
        assert_eq!(report.backend, "github:kokkos/kokkos");
        assert_eq!(report.newest.as_deref(), Some("5.2.2"));
    }

    #[test]
    fn hash_suffixed_versions_compare_on_their_numbers() {
        let report = decide(
            "MMseqs2",
            "18-8cc5c",
            "github:soedinglab/MMseqs2",
            vec![
                Candidate {
                    raw: "18-8cc5c".into(),
                    version: "18-8cc5c".into(),
                    dropped: None,
                },
                Candidate {
                    raw: "17-b804f".into(),
                    version: "17-b804f".into(),
                    dropped: None,
                },
            ],
            None,
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("18-8cc5c"));
        assert_eq!(report.state, "current");
    }

    #[test]
    fn a_pin_wins_and_keeps_its_reason() {
        let ovs = parse_overrides(
            "[SoX]\npin = \"14.4.2\"\nreason = \"upstream dormant; 14.8 is a fork\"\n",
        )
        .unwrap();
        let report = decide(
            "SoX",
            "14.4.2",
            "directory:x",
            vec![Candidate {
                raw: "14.8.1".into(),
                version: "14.8.1".into(),
                dropped: None,
            }],
            ovs.get("SoX"),
        )
        .unwrap();
        assert_eq!(report.state, "pinned");
        assert_eq!(report.newest.as_deref(), Some("14.4.2"));
        assert!(render_text(&report).contains("upstream dormant"));
    }

    #[test]
    fn trailing_release_letters_are_not_prereleases() {
        for v in ["0.15.1b", "3.6b", "12.1.1.post0", "2.7.11b", "20260512"] {
            assert!(!is_prerelease(v), "{v}");
        }
        for v in [
            "1.0b1",
            "3.0a2",
            "5.0.0rc1",
            "2.0-beta",
            "1.2.dev3",
            "4.0.0-alpha.1",
        ] {
            assert!(is_prerelease(v), "{v}");
        }
    }

    #[test]
    fn builtin_overrides_parse() {
        let ovs = parse_overrides(BUILTIN_OVERRIDES).unwrap();
        assert!(ovs.contains_key("GStreamer"));
    }

    fn recipe(text: &str) -> ResolvedEasyconfig {
        crate::eb_parse::resolve_easyconfig_str(text).unwrap()
    }

    const NV: &str = "https://developer.download.nvidia.com/compute";

    #[test]
    fn cuda_reads_the_redistrib_release_labels() {
        let r = recipe("name = 'CUDA'\nversion = '13.4.1'\nhomepage = 'https://developer.nvidia.com/cuda-toolkit'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://developer.download.nvidia.com/compute/cuda/%(version)s/local_installers/']\nsources = ['cuda_%(version)s_linux.run']\n");
        assert_eq!(
            detect_backend(&r, None),
            Some(Backend::NvidiaRedist {
                product: "cuda".into(),
                component: None
            })
        );
        let page = "<a href='redistrib_13.3.1.json'>redistrib_13.3.1.json</a> <a href='redistrib_13.4.1.json'>x</a> <a href='redistrib_13.4.3.json'>y</a>";
        let report = latest(
            &r,
            &BTreeMap::new(),
            &client(&[(format!("{NV}/cuda/redist/").as_str(), page)]),
        )
        .unwrap();
        assert_eq!(report.backend, "nvidia-redist:cuda");
        assert_eq!(report.newest.as_deref(), Some("13.4.3"));
        assert_eq!(report.state, "behind");
    }

    #[test]
    fn cudnn_takes_its_own_version_from_the_newest_manifest() {
        let r = recipe("name = 'cuDNN'\nversion = '9.23.0.39'\nhomepage = 'https://developer.nvidia.com/cudnn'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/linux-%(cudnnarch)s/']\nsources = ['%(namelower)s-linux-%(cudnnarch)s-%(version)s_cuda13-archive.tar.xz']\n");
        assert_eq!(
            detect_backend(&r, None),
            Some(Backend::NvidiaRedist {
                product: "cudnn".into(),
                component: Some("cudnn".into())
            })
        );
        let page = "redistrib_9.23.0.json redistrib_9.27.0.json redistrib_9.26.1.json";
        let manifest = r#"{"release_label": "9.27.0", "cudnn": {"version": "9.27.0.42"}}"#;
        let pages = [
            (format!("{NV}/cudnn/redist/"), page),
            (format!("{NV}/cudnn/redist/redistrib_9.27.0.json"), manifest),
        ];
        let pages: Vec<(&str, &str)> = pages.iter().map(|(u, b)| (u.as_str(), *b)).collect();
        let report = latest(&r, &BTreeMap::new(), &client(&pages)).unwrap();
        assert_eq!(report.newest.as_deref(), Some("9.27.0.42"));
        assert_eq!(report.state, "behind");

        // A manifest without the library's version is not a guess at one.
        let pages = [
            (format!("{NV}/cudnn/redist/"), page),
            (
                format!("{NV}/cudnn/redist/redistrib_9.27.0.json"),
                r#"{"release_label": "9.27.0"}"#,
            ),
        ];
        let pages: Vec<(&str, &str)> = pages.iter().map(|(u, b)| (u.as_str(), *b)).collect();
        let report = latest(&r, &BTreeMap::new(), &client(&pages)).unwrap();
        assert_eq!(report.state, "unknown");
        assert_eq!(report.newest, None);
    }

    #[test]
    fn an_unreadable_release_directory_falls_back_to_a_parent_listing() {
        let r = recipe("name = 'Score-P'\nversion = '10.1'\nhomepage = 'https://www.score-p.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://perftools.example.org/cicd/scorep/tags/scorep-%(version)s']\nsources = ['scorep-%(version)s.tar.gz']\n");
        let page = r#"<a href="tags/scorep-11.0/scorep-11.0.tar.gz">a</a> <a href="tags/scorep-11.1-rc1/scorep-11.1-rc1.tar.gz">b</a> <a href="tags/scorep-10.1/scorep-10.1.tar.gz">c</a>"#;
        let report = latest(
            &r,
            &BTreeMap::new(),
            &client(&[("https://perftools.example.org/cicd/scorep/", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("11.0"));
        assert_eq!(report.state, "behind");

        // Nothing readable at the URL or its two parents stays an error.
        let report = latest(&r, &BTreeMap::new(), &client(&[])).unwrap();
        assert_eq!(report.state, "unknown");
        assert!(report
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("scorep-10.1"));
    }

    #[test]
    fn versioned_directories_linked_by_a_longer_path_are_walked() {
        let r = recipe("name = 'hwloc-CUDA'\nversion = '2.13.0'\nhomepage = 'https://www.open-mpi.org/projects/hwloc/'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://www.example.org/software/hwloc/v%(version_major_minor)s/downloads/']\nsources = ['hwloc-%(version)s.tar.gz']\n");
        let root = "https://www.example.org/software/hwloc/";
        let parent = r#"<a href="doc/v2.15.0/index.html">d</a> <a href="../../software/hwloc/v2.13/">a</a> <a href="../../software/hwloc/v2.14/">b</a> <a href="../../software/hwloc/v2.15/">c</a> <a href="https://elsewhere.org/v9.9/">e</a>"#;
        let pages = [
            (
                format!("{root}v2.13/"),
                "hwloc-2.13.0.tar.gz hwloc-2.13.0.tar.bz2",
            ),
            (root.to_string(), parent),
            (
                format!("{root}v2.15/"),
                "hwloc-2.15.0.tar.gz hwloc-2.15.0rc1.tar.gz",
            ),
            (format!("{root}v2.14/"), "hwloc-2.14.0.tar.gz"),
        ];
        let pages: Vec<(&str, &str)> = pages.iter().map(|(u, b)| (u.as_str(), *b)).collect();
        let report = latest(&r, &BTreeMap::new(), &client(&pages)).unwrap();
        assert_eq!(report.newest.as_deref(), Some("2.15.0"));
        assert!(!report
            .candidates
            .iter()
            .any(|c| c.version.starts_with("9.9")));
    }

    #[test]
    fn a_download_query_reads_the_page_without_it() {
        let r = recipe("name = 'SIONlib'\nversion = '1.7.7'\nhomepage = 'https://example.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://apps.example.org/sionlib/download.php?version=%(version)sl']\nsources = ['%(namelower)s-%(version)sl.tar.gz']\n");
        let page = "sionlib-1.7.6l.tar.gz sionlib-1.7.7.tar.gz sionlib-1.7.7l.tar.gz sionlib-2.0.0-rc.1l.tar.gz";
        let report = latest(
            &r,
            &BTreeMap::new(),
            &client(&[("https://apps.example.org/sionlib/download.php", page)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("1.7.7"));
        assert_eq!(report.state, "current");
    }

    #[test]
    fn any_gitlab_archive_url_reads_the_gitlab_api() {
        let r = recipe("name = 'libcerf'\nversion = '3.3'\nhomepage = 'https://jugit.example.de/mlz/libcerf'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://jugit.example.de/mlz/libcerf/-/archive/v%(version)s/']\nsources = ['libcerf-v%(version)s.tar.gz']\n");
        assert_eq!(
            detect_backend(&r, None),
            Some(Backend::Gitlab {
                host: "jugit.example.de".into(),
                path: "mlz/libcerf".into()
            })
        );
        let doc = r#"[{"tag_name": "v3.9"}, {"tag_name": "v3.3"}]"#;
        let report = latest(
            &r,
            &BTreeMap::new(),
            &client(&[(
                "https://jugit.example.de/api/v4/projects/mlz%2Flibcerf/releases?per_page=100",
                doc,
            )]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("3.9"));
    }

    #[test]
    fn a_tag_prefix_the_name_contains_is_stripped() {
        assert_eq!(tag_version("gd-2.3.3", "libgd"), "2.3.3");
        assert_eq!(tag_version("hwloc-2.15.0", "hwloc-CUDA"), "2.15.0");
        assert_eq!(tag_version("hwloc-v0.9.2", "hwloc-CUDA"), "0.9.2");
        assert_eq!(tag_version("scorep-10.1", "Score-P"), "10.1");
        // Another product in the same repository keeps its prefix.
        assert_eq!(tag_version("infernal-1.1.5", "HMMER"), "infernal-1.1.5");
        assert_eq!(tag_version("hwloc-2.15-branch", "hwloc"), "2.15-branch");
        assert_eq!(tag_version("nccl4py-v0.6.0", "NCCL"), "4py-v0.6.0");
    }

    #[test]
    fn a_python_package_reads_pypi_before_its_github_homepage() {
        let r = recipe("easyblock = 'PythonPackage'\nname = 'pytest-shard'\nversion = '0.1.2'\nhomepage = 'https://github.com/AdamGleave/pytest-shard'\ndescription = 'x'\ntoolchain = SYSTEM\nsources = [SOURCE_TAR_GZ]\n");
        assert_eq!(
            detect_backend(&r, None),
            Some(Backend::Pypi {
                project: "pytest-shard".into()
            })
        );
    }

    #[test]
    fn a_source_dict_with_its_own_urls_names_the_backend() {
        let r = recipe("easyblock = 'Tarball'\nname = 'ecBuild'\nversion = '3.14.2'\nhomepage = 'https://ecbuild.readthedocs.io/'\ndescription = 'x'\ntoolchain = SYSTEM\ngithub_account = 'ecmwf'\nsources = [{'source_urls': [GITHUB_SOURCE], 'filename': '%(version)s.tar.gz'}]\n");
        assert_eq!(
            detect_backend(&r, None),
            Some(Backend::Github {
                owner: "ecmwf".into(),
                repo: "ecBuild".into()
            })
        );
    }

    #[test]
    fn sourceforge_reads_the_project_file_feed() {
        let gts = recipe("name = 'GTS'\nversion = '0.7.6'\nhomepage = 'https://gts.sourceforge.net/'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = [SOURCEFORGE_SOURCE]\nsources = [SOURCELOWER_TAR_GZ]\n");
        assert_eq!(
            detect_backend(&gts, None),
            Some(Backend::Directory {
                url: "https://sourceforge.net/projects/gts/rss".into(),
                prefix: "gts-".into(),
                suffix: ".tar.gz".into(),
            })
        );
        // gnuplot's (url, 'download') source_urls pair is not kept by the
        // parser; the homepage names the project.
        let gnuplot = recipe("name = 'gnuplot'\nversion = '6.0.5'\nhomepage = 'http://gnuplot.sourceforge.net'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = [('https://sourceforge.net/projects/%(name)s/files/%(name)s/%(version)s', 'download')]\nsources = [SOURCE_TAR_GZ]\n");
        let feed = "<title><![CDATA[/gnuplot/6.0.6/gnuplot-6.0.6.tar.gz]]></title> <title><![CDATA[/gnuplot/testing/gnuplot-6.0.6-testing.tar.gz]]></title> <title><![CDATA[/gnuplot/6.0.5/gnuplot-6.0.5.tar.gz]]></title>";
        let report = latest(
            &gnuplot,
            &BTreeMap::new(),
            &client(&[("https://sourceforge.net/projects/gnuplot/rss", feed)]),
        )
        .unwrap();
        assert_eq!(report.newest.as_deref(), Some("6.0.6"));
        assert_eq!(report.state, "behind");
    }

    #[test]
    fn a_recipe_with_components_is_a_bundle() {
        let r = recipe("name = 'gnupg-bundle'\nversion = '20260518'\nhomepage = 'https://www.gnupg.org/software/index.html'\ndescription = 'x'\ntoolchain = SYSTEM\ndefault_easyblock = 'ConfigureMake'\ncomponents = [('npth', '1.8', {'sources': [SOURCE_TAR_BZ2]})]\n");
        let report = latest(&r, &BTreeMap::new(), &client(&[])).unwrap();
        assert_eq!(report.state, "unknown");
        assert!(
            report.reason.as_deref().unwrap_or("").contains("bundle"),
            "{:?}",
            report.reason
        );
    }

    #[test]
    fn an_override_directory_takes_templates_and_keeps_the_recipe_file_name() {
        let r = recipe("name = 'ParaView'\nversion = '6.1.1'\nhomepage = 'https://www.paraview.org'\ndescription = 'x'\ntoolchain = SYSTEM\nsource_urls = ['https://www.paraview.org/paraview-downloads/download.php?version=v%(version_major_minor)s']\nsources = ['%(name)s-v%(version)s.tar.gz']\n");
        let ovs = parse_overrides("[ParaView]\nbackend = \"directory\"\nproject = \"https://files.example.org/v%(version_major_minor)s/\"\nreason = \"x\"\n").unwrap();
        let pages = [
            (
                "https://files.example.org/v6.1/",
                "ParaView-v6.1.0.tar.gz ParaView-v6.1.1.tar.gz",
            ),
            (
                "https://files.example.org/",
                r#"<a href="v6.0/">a</a> <a href="v6.1/">b</a> <a href="v6.2/">c</a>"#,
            ),
            (
                "https://files.example.org/v6.2/",
                "ParaView-v6.2.0.tar.gz ParaView-v6.2.1-RC1.tar.gz",
            ),
        ];
        let report = latest(&r, &ovs, &client(&pages)).unwrap();
        assert_eq!(report.backend, "directory:https://files.example.org/v6.1/");
        assert_eq!(report.newest.as_deref(), Some("6.2.0"));
    }

    #[test]
    fn an_override_with_no_backend_reports_unknown_with_its_reason() {
        let r = ec(
            "QIIME2",
            "2022.11",
            &["https://data.example.org/distro/core/"],
        );
        let ovs = parse_overrides(
            "[QIIME2]\nbackend = \"none\"\nreason = \"the core distribution ended\"\n",
        )
        .unwrap();
        let report = latest(&r, &ovs, &client(&[])).unwrap();
        assert_eq!(report.state, "unknown");
        assert_eq!(report.backend, "none");
        assert_eq!(
            report.reason.as_deref(),
            Some("the core distribution ended")
        );
    }
}
