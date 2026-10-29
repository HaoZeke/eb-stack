//! The newest stable upstream release of an EasyBuild recipe.
//!
//! The backend is read from the recipe's own `source_urls`: PyPI, GitHub,
//! GitLab, CRAN, crates.io, or a plain HTTP directory. A distribution
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
        }
    }
}

/// Per-package settings, keyed by EasyBuild name in the overrides TOML.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// `pypi`, `github`, `gitlab`, `cran`, `crates`, or `directory`.
    pub backend: Option<String>,
    /// Project for the backend: `owner/repo`, a PyPI or CRAN name, or a URL.
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
        if let (Some(kind), Some(project)) = (ov.backend.as_deref(), ov.project.as_deref()) {
            return backend_from_override(kind, project, &ec.name);
        }
    }
    let urls: Vec<&str> = ec
        .source_urls
        .iter()
        .map(String::as_str)
        .chain(ec.homepage.as_deref())
        .collect();
    let gh = Regex::new(r"^https?://(?:www\.)?github\.com/([^/]+)/([^/#?]+)").unwrap();
    let gl = Regex::new(r"^https?://(gitlab\.[^/]+)/(.+?)/(?:-/|archive|uploads|$)").unwrap();
    for url in &urls {
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
        if let Some(c) = gh.captures(url) {
            return Some(Backend::Github {
                owner: c[1].to_string(),
                repo: c[2].trim_end_matches(".git").to_string(),
            });
        }
        if let Some(c) = gl.captures(url) {
            return Some(Backend::Gitlab {
                host: c[1].to_string(),
                path: c[2].to_string(),
            });
        }
    }
    if ec.source_urls.is_empty()
        && matches!(
            ec.easyblock.as_deref(),
            Some("PythonPackage" | "CargoPythonPackage")
        )
    {
        // The easyblock adds the PyPI source URL itself.
        return Some(Backend::Pypi {
            project: ec.name.clone(),
        });
    }
    let first = ec.source_urls.first()?;
    let prefix = ec
        .source_files
        .first()
        .map(|f| file_prefix(&f.filename, &ec.version))
        .unwrap_or_else(|| format!("{}-", ec.name));
    Some(Backend::Directory {
        url: first.clone(),
        prefix,
    })
}

fn backend_from_override(kind: &str, project: &str, name: &str) -> Option<Backend> {
    Some(match kind {
        "pypi" => Backend::Pypi {
            project: project.to_string(),
        },
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
        "cran" => Backend::Cran {
            package: project.to_string(),
        },
        "crates" => Backend::Crates {
            name: project.to_string(),
        },
        "directory" => Backend::Directory {
            url: project.to_string(),
            prefix: format!("{name}-"),
        },
        _ => return None,
    })
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
        Backend::Directory { url, prefix } => {
            let body = client.get(url).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut pages = vec![String::from_utf8_lossy(&body).into_owned()];
            for dir in versioned_dirs(url, client) {
                if dir != *url {
                    if let Ok(body) = client.get(&dir) {
                        pages.push(String::from_utf8_lossy(&body).into_owned());
                    }
                }
            }
            // A dash in the version must open another number, so a platform
            // build (`blender-4.0.2-windows-x64.zip`) is not a version.
            let re = Regex::new(&format!(
                r"{}([0-9][0-9A-Za-z._]*?(?:-[0-9][0-9A-Za-z._]*?)*)(?:\.tar\.gz|\.tar\.bz2|\.tar\.xz|\.tgz|\.zip|\.c|\.tar)\b",
                regex::escape(prefix)
            ))?;
            let mut seen = std::collections::BTreeSet::new();
            for html in &pages {
                for c in re.captures_iter(html) {
                    let v = c[1].trim_end_matches(['.', '-', '_']).to_string();
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

/// Why a recipe has no backend.
fn no_backend_reason(ec: &ResolvedEasyconfig) -> &'static str {
    if ec
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
    let Ok(re) = Regex::new(&format!(
        r#"href="(?:\./)?{}(\d+(?:\.\d+)+){}/?""#,
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
    for c in candidates.iter_mut().filter(|c| c.dropped.is_none()) {
        let v = c.version.as_str();
        c.dropped = if v.is_empty() || !v.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            Some("not a version".into())
        } else if is_prerelease(v) {
            Some("pre-release".into())
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
    survivors.sort_by(|a, b| cmp_version(&b.version, &a.version).then_with(|| a.raw.cmp(&b.raw)));
    let newest = survivors.first().map(|c| c.version.clone());
    let reason = ov.and_then(|o| o.reason.clone());
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
        if report.reason.is_none() {
            report.reason = Some(no_backend_reason(ec).into());
        }
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
}
