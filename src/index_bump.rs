//! Versions and checksums an index can decide after a version bump.
//!
//! Two EasyBuild mechanisms, and nothing past them:
//!
//! * `easybuild/easyblocks/generic/cargo.py` `generate_crate_list` reads
//!   `Cargo.lock` and writes `crates` as `(name, version)` for every crates.io
//!   package (git crates keep url and rev). The checksum EasyBuild records for
//!   `{name}-{version}.tar.gz` is the lock's `checksum`, which is the digest
//!   crates.io published for that `.crate`.
//! * `easy_update` moves an `exts_list` entry when the extension *is* the
//!   package being bumped, or when the release the recipe names no longer
//!   imports on the bundled Python or R (`requires_python` / CRAN `Depends`).
//!   Other pins stay: `easy_update` would also jump them to today's newest
//!   release and add their dependencies, which is a maintainer decision, not
//!   something the recipe's index has already fixed. `eb --inject-checksums`
//!   then hashes the sdist that entry will actually download.
//!
//! Extensions are not added, removed, or reordered. `configopts`, patches, and
//! sanity checks are left untouched.

use crate::eb_emit::easyconfig_list_span;
use crate::eb_parse::ResolvedEasyconfig;
use crate::package::{Residual, ResidualSeverity, ResidualStage};
use crate::registry::RegistryClient;
use crate::version::{cmp_version, matches_req};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

/// One archive already downloaded for this bump.
pub(crate) struct FetchedArchive<'a> {
    /// Name the recipe saves the archive under.
    pub filename: &'a str,
    /// The bytes.
    pub bytes: &'a [u8],
}

/// Apply crate-list and extension rewrites to an emitted recipe.
pub(crate) fn rewrite_fetched_recipe(
    text: &str,
    prior: Option<&ResolvedEasyconfig>,
    archives: &[FetchedArchive<'_>],
    client: &dyn RegistryClient,
) -> (String, Vec<Residual>) {
    let mut residuals = Vec::new();
    let Ok(recipe) = crate::eb_parse::resolve_easyconfig_str(text) else {
        return (text.to_string(), residuals);
    };
    let mut text = text.to_string();
    if list_body(&text, "crates").is_some() {
        match rewrite_crates(&text, &recipe, archives, client) {
            Ok((rewritten, notes)) => {
                text = rewritten;
                residuals.extend(notes);
            }
            Err(reason) => residuals.push(residual(
                "crates:lock",
                "crates",
                ResidualSeverity::Blocking,
                "crates list was not rewritten from Cargo.lock",
                Some(reason),
            )),
        }
    }
    if list_body(&text, "exts_list").is_some() {
        let (rewritten, notes) = rewrite_extensions(&text, &recipe, prior, client);
        text = rewritten;
        residuals.extend(notes);
    }
    (text, residuals)
}

struct LockedCrate {
    name: String,
    version: String,
    checksum: Option<String>,
    git: Option<GitSource>,
}

struct GitSource {
    url: String,
    rev: String,
}

fn rewrite_crates(
    text: &str,
    recipe: &ResolvedEasyconfig,
    archives: &[FetchedArchive<'_>],
    client: &dyn RegistryClient,
) -> Result<(String, Vec<Residual>), String> {
    let mut residuals = Vec::new();
    let archive = cargo_archive(text, recipe, archives, client)?;
    let files = files_in_archive(&archive).map_err(|error| {
        format!("could not unpack the source that should carry Cargo.lock: {error}")
    })?;
    let (manifest, lock) = cargo_sources(&files).ok_or_else(|| {
        "no Cargo.lock in the fetched source; the crate list is whatever \
         `cargo generate-lockfile` would write, which this bump does not run"
            .to_string()
    })?;
    let (app_in_cratesio, crates) = locked_crates(&manifest, &lock)?;
    let old_files = crate_filenames_in_list(text);
    let mut rewritten = replace_list_body(
        text,
        "crates",
        &render_crates_body(app_in_cratesio, &crates),
    )
    .ok_or_else(|| "crates list could not be rewritten".to_string())?;
    if list_body(&rewritten, "checksums").is_some() {
        rewritten = rewrite_crate_checksums(&rewritten, &old_files, &crates);
    } else if crates.iter().any(|crate_| crate_.checksum.is_some()) {
        residuals.push(residual(
            "crates:checksums",
            "crates",
            ResidualSeverity::Blocking,
            "crate digests from Cargo.lock were not written",
            Some("the recipe has no top-level checksums list".into()),
        ));
    }
    let git_crates = crates
        .iter()
        .filter(|crate_| crate_.git.is_some())
        .map(|crate_| crate_.name.as_str())
        .collect::<Vec<_>>();
    if !git_crates.is_empty() {
        residuals.push(residual(
            "crates:git",
            "crates",
            ResidualSeverity::Judgment,
            "git crate checksums were not taken from Cargo.lock",
            Some(format!(
                "EasyBuild hashes the tar.xz it builds from the git revision; the lock digest is a different artifact: {}",
                git_crates.join(", ")
            )),
        ));
    }
    for crate_ in &crates {
        if crate_.git.is_none() && crate_.checksum.is_none() {
            residuals.push(residual(
                &format!("crates:{}", crate_.name),
                "crates",
                ResidualSeverity::Blocking,
                &format!(
                    "crate {} {} has no checksum in Cargo.lock",
                    crate_.name, crate_.version
                ),
                None,
            ));
        }
    }
    Ok((rewritten, residuals))
}

fn cargo_archive(
    text: &str,
    recipe: &ResolvedEasyconfig,
    archives: &[FetchedArchive<'_>],
    client: &dyn RegistryClient,
) -> Result<Vec<u8>, String> {
    let mut unpack_errors = Vec::new();
    for archive in archives {
        match files_in_archive(archive.bytes) {
            Ok(files) if cargo_sources(&files).is_some() => return Ok(archive.bytes.to_vec()),
            Ok(_) => {}
            Err(error) => unpack_errors.push(format!("{}: {error}", archive.filename)),
        }
    }
    // A Python bundle's lock lives in the extension sdist, not in `sources`.
    if let Some(index) = extension_index(text, recipe) {
        let name = recipe.name.clone();
        let version = recipe.version.clone();
        let draft = ExtDraft {
            raw: String::new(),
            name: name.clone(),
            version: version.clone(),
            tracks_recipe: true,
            source_urls: Vec::new(),
            source_templates: Vec::new(),
        };
        if let Ok(bytes) = download_extension(client, index, &draft, &version, recipe) {
            return Ok(bytes);
        }
    }
    if unpack_errors.is_empty() {
        Err("no fetched archive contains Cargo.lock".into())
    } else {
        Err(format!(
            "no fetched archive contains Cargo.lock ({})",
            unpack_errors.join("; ")
        ))
    }
}

fn files_in_archive(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let owned;
    let tar_bytes: &[u8] = if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let mut buf = Vec::new();
        decoder
            .read_to_end(&mut buf)
            .map_err(|error| error.to_string())?;
        owned = buf;
        &owned
    } else {
        bytes
    };
    let mut archive = tar::Archive::new(tar_bytes);
    let entries = archive.entries().map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|error| error.to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
        files.push((path, data));
    }
    Ok(files)
}

fn cargo_sources(files: &[(String, Vec<u8>)]) -> Option<(String, String)> {
    let mut locks: Vec<&(String, Vec<u8>)> = files
        .iter()
        .filter(|(path, _)| {
            (path == "Cargo.lock" || path.ends_with("/Cargo.lock")) && !path.contains("/vendor/")
        })
        .collect();
    locks.sort_by_key(|(path, _)| path.len());
    let (lock_path, lock_bytes) = locks.first()?;
    let lock = String::from_utf8(lock_bytes.clone()).ok()?;
    let manifest_path = lock_path.replace("Cargo.lock", "Cargo.toml");
    let manifest = files
        .iter()
        .find(|(path, _)| path == &manifest_path)
        .and_then(|(_, bytes)| String::from_utf8(bytes.clone()).ok())
        .unwrap_or_default();
    Some((manifest, lock))
}

fn locked_crates(manifest: &str, lock: &str) -> Result<(bool, Vec<LockedCrate>), String> {
    let lock_value: toml::Value = toml::from_str(lock).map_err(|error| error.to_string())?;
    let manifest_value: toml::Value = if manifest.is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(manifest).map_err(|error| error.to_string())?
    };
    let app = manifest_value
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .unwrap_or("")
        .to_string();
    let packages = lock_value
        .get("package")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "Cargo.lock has no [[package]] entries".to_string())?;
    let mut app_in_cratesio = false;
    let mut crates = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let version = package
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || version.is_empty() {
            continue;
        }
        let Some(source) = package.get("source").and_then(|value| value.as_str()) else {
            continue;
        };
        let checksum = package
            .get("checksum")
            .and_then(|value| value.as_str())
            .filter(|value| is_sha256(value))
            .map(str::to_string);
        if source.contains("crates.io") {
            if name == app {
                app_in_cratesio = true;
                continue;
            }
            crates.push(LockedCrate {
                name,
                version,
                checksum,
                git: None,
            });
            continue;
        }
        if source.starts_with("git+") || source.contains("git+") {
            match parse_git_source(source) {
                Ok((url, rev)) => crates.push(LockedCrate {
                    name,
                    version,
                    checksum,
                    git: Some(GitSource { url, rev }),
                }),
                Err(error) => {
                    return Err(format!("crate {name}: {error}"));
                }
            }
        }
    }
    crates.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.version.cmp(&right.version))
            .then_with(|| match (&left.git, &right.git) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => a.url.cmp(&b.url).then(a.rev.cmp(&b.rev)),
                (None, None) => std::cmp::Ordering::Equal,
            })
    });
    Ok((app_in_cratesio, crates))
}

/// `git+https://host/repo?rev=short#long` the way `generate_crate_list` reads it.
fn parse_git_source(source: &str) -> Result<(String, String), String> {
    let stripped = source
        .strip_prefix("git+")
        .or_else(|| source.strip_prefix("registry+"))
        .unwrap_or(source);
    let (before_hash, fragment) = stripped.split_once('#').unwrap_or((stripped, ""));
    let (base, query) = before_hash.split_once('?').unwrap_or((before_hash, ""));
    if fragment.is_empty() {
        return Err(format!("no revision in git source {source}"));
    }
    let rev_query = query.split('&').find_map(|part| part.strip_prefix("rev="));
    let rev = match rev_query {
        Some(query_rev) if query_rev != fragment => {
            if fragment.starts_with(query_rev) || query_rev.starts_with(fragment) {
                query_rev.to_string()
            } else {
                return Err(format!(
                    "revision mismatch in {source}: {query_rev} vs {fragment}"
                ));
            }
        }
        _ => fragment.to_string(),
    };
    Ok((base.to_string(), rev))
}

fn crate_filename(crate_: &LockedCrate) -> String {
    match &crate_.git {
        Some(git) => {
            let short = &git.rev[..git.rev.len().min(8)];
            format!("{}-{}-{short}.tar.xz", crate_.name, crate_.version)
        }
        None => format!("{}-{}.tar.gz", crate_.name, crate_.version),
    }
}

fn render_crate_tuple(crate_: &LockedCrate) -> String {
    match &crate_.git {
        Some(git) => format!(
            "('{}', '{}', '{}', '{}')",
            crate_.name, crate_.version, git.url, git.rev
        ),
        None => format!("('{}', '{}')", crate_.name, crate_.version),
    }
}

fn render_crates_body(app_in_cratesio: bool, crates: &[LockedCrate]) -> String {
    let mut out = String::from("\n");
    if app_in_cratesio {
        out.push_str("    (name, version),\n");
    }
    for crate_ in crates {
        out.push_str("    ");
        out.push_str(&render_crate_tuple(crate_));
        out.push_str(",\n");
    }
    out
}

fn crate_filenames_in_list(text: &str) -> BTreeSet<String> {
    let Some(body) = list_body(text, "crates") else {
        return BTreeSet::new();
    };
    let mut names = BTreeSet::new();
    let pattern = regex::Regex::new(
        r#"\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"](?:\s*,\s*['"]([^'"]*)['"]\s*,\s*['"]([^'"]*)['"])?\s*,?\s*\)"#,
    );
    let Ok(pattern) = pattern else {
        return names;
    };
    for caps in pattern.captures_iter(&body) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if let (Some(url), Some(rev)) = (caps.get(3), caps.get(4)) {
            let short = &rev.as_str()[..rev.as_str().len().min(8)];
            names.insert(format!("{name}-{version}-{short}.tar.xz"));
            let _ = url;
        } else if !name.is_empty() {
            names.insert(format!("{name}-{version}.tar.gz"));
        }
    }
    names
}

fn rewrite_crate_checksums(
    text: &str,
    old_files: &BTreeSet<String>,
    crates: &[LockedCrate],
) -> String {
    let Some(body) = list_body(text, "checksums") else {
        return text.to_string();
    };
    let entries = split_top_level(&body);
    // Registry checksums are the `.crate` bytes Cargo.lock records, which
    // EasyBuild saves as `{name}-{version}.tar.gz`. A git crate's lock digest
    // is not the `{name}-{version}-{rev}.tar.xz` EasyBuild builds from the
    // revision, so it is not written.
    let rendered = crates
        .iter()
        .filter(|crate_| crate_.git.is_none())
        .filter_map(|crate_| {
            let checksum = crate_.checksum.as_deref()?;
            Some(format!("{{'{}': '{checksum}'}}", crate_filename(crate_)))
        })
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut placed = false;
    for entry in entries {
        if entry_names_crate(&entry, old_files) {
            if !placed {
                out.extend(rendered.iter().cloned());
                placed = true;
            }
        } else {
            out.push(entry);
        }
    }
    if !placed {
        out.extend(rendered);
    }
    replace_list_body(text, "checksums", &render_list_body(&out))
        .unwrap_or_else(|| text.to_string())
}

fn entry_names_crate(entry: &str, old_files: &BTreeSet<String>) -> bool {
    let Ok(pattern) = regex::Regex::new(r#"['"]([^'"]+)['"]\s*:"#) else {
        return false;
    };
    for caps in pattern.captures_iter(entry) {
        if old_files.contains(caps.get(1).map(|m| m.as_str()).unwrap_or("")) {
            return true;
        }
    }
    false
}

#[derive(Clone)]
struct ExtDraft {
    raw: String,
    name: String,
    version: String,
    tracks_recipe: bool,
    source_urls: Vec<String>,
    source_templates: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtIndex {
    Pypi,
    Cran,
}

fn rewrite_extensions(
    text: &str,
    recipe: &ResolvedEasyconfig,
    prior: Option<&ResolvedEasyconfig>,
    client: &dyn RegistryClient,
) -> (String, Vec<Residual>) {
    let Some(index) = extension_index(text, recipe) else {
        return (text.to_string(), Vec::new());
    };
    let Some(body) = list_body(text, "exts_list") else {
        return (text.to_string(), Vec::new());
    };
    let prior_versions: BTreeMap<String, String> = prior
        .map(|recipe| {
            recipe
                .exts_list
                .iter()
                .map(|ext| (ext.name.clone(), ext.version.clone()))
                .collect()
        })
        .unwrap_or_default();
    let prior_python = prior.and_then(|recipe| named_dep(recipe, "Python"));
    let prior_r = prior.and_then(|recipe| named_dep(recipe, "R"));
    let python = named_dep(recipe, "Python");
    let r_version = named_dep(recipe, "R");
    let interpreter_changed = match index {
        ExtIndex::Pypi => prior_python != python,
        ExtIndex::Cran => prior_r != r_version,
    };
    let old_recipe_version = prior.map(|recipe| recipe.version.as_str());
    let mut residuals = Vec::new();
    let mut entries = Vec::new();
    let mut changed = false;
    for raw in split_top_level(&body) {
        let Some(mut draft) = parse_ext_tuple(&raw, recipe, old_recipe_version) else {
            entries.push(raw);
            continue;
        };
        let original = draft.raw.clone();
        let previous = prior_versions.get(&draft.name).map(String::as_str);
        if draft.tracks_recipe {
            if let Some(old_version) = old_recipe_version {
                if draft.version == old_version && draft.version != recipe.version {
                    draft.raw =
                        replace_version_literal(&draft.raw, &draft.version, &recipe.version);
                    draft.version = recipe.version.clone();
                }
            }
        } else if interpreter_changed {
            match move_if_incompatible(client, index, &draft, python.as_deref(), r_version.as_deref()) {
                Ok(Some(version)) => {
                    draft.raw = replace_version_literal(&draft.raw, &draft.version, &version);
                    draft.version = version;
                }
                Ok(None) => {}
                Err(error) => residuals.push(residual(
                    &format!("extension:{}", draft.name),
                    "extension",
                    ResidualSeverity::Judgment,
                    &format!(
                        "extension {} version left at {}: the index did not decide a compatible release",
                        draft.name, draft.version
                    ),
                    Some(error),
                )),
            }
        }
        let version_changed = previous != Some(draft.version.as_str());
        if version_changed {
            if let Some(previous) = previous {
                match download_extension(client, index, &draft, &draft.version, recipe) {
                    Ok(bytes) => {
                        let digest = sha256_hex(&bytes);
                        let updated = replace_extension_checksum(
                            &draft.raw,
                            previous,
                            &draft.version,
                            &digest,
                        );
                        if updated.contains(&digest) {
                            draft.raw = updated;
                        } else {
                            residuals.push(residual(
                                &format!("extension:{}:checksum", draft.name),
                                "extension",
                                ResidualSeverity::Judgment,
                                &format!(
                                    "extension {} {} checksum was fetched but the entry does not spell it as a literal",
                                    draft.name, draft.version
                                ),
                                None,
                            ));
                        }
                    }
                    Err(error) => {
                        residuals.push(residual(
                            &format!("extension:{}:checksum", draft.name),
                            "extension",
                            ResidualSeverity::Blocking,
                            &format!(
                                "extension {} {} checksum was not fetched",
                                draft.name, draft.version
                            ),
                            Some(error),
                        ));
                        draft.raw =
                            replace_extension_checksum(&draft.raw, previous, &draft.version, "");
                    }
                }
            }
        }
        if draft.raw != original {
            changed = true;
        }
        entries.push(draft.raw);
    }
    if !changed {
        return (text.to_string(), residuals);
    }
    let Some(rewritten) = replace_list_body(text, "exts_list", &render_list_body(&entries)) else {
        return (text.to_string(), residuals);
    };
    (rewritten, residuals)
}

fn extension_index(text: &str, recipe: &ResolvedEasyconfig) -> Option<ExtIndex> {
    let block = recipe.easyblock.as_deref().unwrap_or("");
    if block.ends_with("PythonPackage") || block.ends_with("PythonBundle") {
        return Some(ExtIndex::Pypi);
    }
    if block == "RPackage" || block.ends_with("RPackage") {
        return Some(ExtIndex::Cran);
    }
    let defaults = assignment_text(text, "exts_default_options").unwrap_or_default();
    let class = assignment_text(text, "exts_defaultclass").unwrap_or_default();
    let haystack = format!("{defaults}\n{class}");
    if haystack.to_ascii_lowercase().contains("pypi")
        || class.contains("PythonPackage")
        || class.contains("PythonBundle")
    {
        return Some(ExtIndex::Pypi);
    }
    if haystack.to_ascii_lowercase().contains("cran") || class.contains("RPackage") {
        return Some(ExtIndex::Cran);
    }
    None
}

fn named_dep(recipe: &ResolvedEasyconfig, name: &str) -> Option<String> {
    recipe
        .dependencies
        .iter()
        .chain(recipe.builddependencies.iter())
        .find(|dep| dep.name.eq_ignore_ascii_case(name))
        .map(|dep| dep.version.clone())
}

fn move_if_incompatible(
    client: &dyn RegistryClient,
    index: ExtIndex,
    draft: &ExtDraft,
    python: Option<&str>,
    r_version: Option<&str>,
) -> Result<Option<String>, String> {
    match index {
        ExtIndex::Pypi => {
            let Some(python) = python else {
                return Err("no Python dependency to test requires_python against".into());
            };
            if pypi_accepts(client, &draft.name, &draft.version, python)? {
                return Ok(None);
            }
            let newest = pypi_newest_sdist(client, &draft.name, python)?;
            if newest == draft.version {
                Ok(None)
            } else {
                Ok(Some(newest))
            }
        }
        ExtIndex::Cran => {
            // crandb publishes the current release only, so an older pin's
            // Depends line is not something this client can see. Leave the pin.
            // The package's own extension still tracks `version` above, and
            // its tarball is hashed from CRAN when that version changed.
            let _ = (client, draft, r_version);
            Ok(None)
        }
    }
}

fn pypi_accepts(
    client: &dyn RegistryClient,
    name: &str,
    version: &str,
    python: &str,
) -> Result<bool, String> {
    let value = pypi_json(
        client,
        &format!("https://pypi.org/pypi/{name}/{version}/json"),
    )?;
    let requires = requires_python(&value);
    Ok(specifier_accepts(requires.as_deref(), python))
}

fn pypi_newest_sdist(
    client: &dyn RegistryClient,
    name: &str,
    python: &str,
) -> Result<String, String> {
    let value = pypi_json(client, &format!("https://pypi.org/pypi/{name}/json"))?;
    let mut best: Option<String> = None;
    let releases = value
        .get("releases")
        .and_then(|releases| releases.as_object())
        .ok_or_else(|| format!("PyPI JSON for {name} has no releases"))?;
    for (version, files) in releases {
        if is_pypi_prerelease(version) {
            continue;
        }
        let Some(files) = files.as_array() else {
            continue;
        };
        let sdists: Vec<&serde_json::Value> = files
            .iter()
            .filter(|file| {
                file.get("packagetype")
                    .and_then(|kind| kind.as_str())
                    .is_some_and(|kind| kind == "sdist")
                    && !file
                        .get("yanked")
                        .and_then(|flag| flag.as_bool())
                        .unwrap_or(false)
            })
            .collect();
        if sdists.is_empty() {
            continue;
        }
        let requires = sdists
            .iter()
            .find_map(|file| file.get("requires_python").and_then(|value| value.as_str()))
            .or_else(|| {
                value
                    .pointer("/info/requires_python")
                    .and_then(|item| item.as_str())
            });
        if !specifier_accepts(requires, python) {
            continue;
        }
        if best
            .as_deref()
            .is_none_or(|current| cmp_version(version, current).is_gt())
        {
            best = Some(version.clone());
        }
    }
    best.ok_or_else(|| format!("no sdist of {name} accepts Python {python}"))
}

fn specifier_accepts(requires: Option<&str>, version: &str) -> bool {
    match requires.map(str::trim).filter(|spec| !spec.is_empty()) {
        None => true,
        Some(spec) => matches_req(version, spec),
    }
}

fn requires_python(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/info/requires_python")
        .and_then(|item| item.as_str())
        .map(str::to_string)
}

fn pypi_json(client: &dyn RegistryClient, url: &str) -> Result<serde_json::Value, String> {
    let bytes = client.get(url).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{url}: {error}"))
}

fn is_pypi_prerelease(version: &str) -> bool {
    let lower = version.to_ascii_lowercase();
    // PEP 440 writes the pre-release tag against the last numeric component:
    // `1.0a1`, `1.0rc1`, `1.0.dev1`.
    regex::Regex::new(r"\d(?:a|b|rc|alpha|beta)\d|\.dev\d")
        .is_ok_and(|pattern| pattern.is_match(&lower))
}

fn download_extension(
    client: &dyn RegistryClient,
    index: ExtIndex,
    draft: &ExtDraft,
    version: &str,
    recipe: &ResolvedEasyconfig,
) -> Result<Vec<u8>, String> {
    if !draft.source_urls.is_empty() {
        let names = if draft.source_templates.is_empty() {
            vec![format!("{}-{version}.tar.gz", draft.name)]
        } else {
            draft
                .source_templates
                .iter()
                .map(|template| expand_ext_template(template, &draft.name, version))
                .collect::<Vec<_>>()
        };
        let mut errors = Vec::new();
        for base in &draft.source_urls {
            if base.contains("%(") && expand_ext_template(base, &draft.name, version).contains("%(")
            {
                errors.push(format!("{base}: unresolved template"));
                continue;
            }
            let base = expand_ext_template(base, &draft.name, version);
            for name in &names {
                let url = join_url(&base, name);
                match client.get(&url) {
                    Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                    Ok(_) => errors.push(format!("{url}: empty body")),
                    Err(error) => errors.push(error.to_string()),
                }
            }
        }
        return Err(errors.join("; "));
    }
    match index {
        ExtIndex::Pypi => download_pypi_sdist(client, &draft.name, version),
        ExtIndex::Cran => download_cran_tarball(client, &draft.name, version, recipe),
    }
}

fn download_pypi_sdist(
    client: &dyn RegistryClient,
    name: &str,
    version: &str,
) -> Result<Vec<u8>, String> {
    let value = pypi_json(
        client,
        &format!("https://pypi.org/pypi/{name}/{version}/json"),
    )?;
    let urls = value
        .get("urls")
        .and_then(|item| item.as_array())
        .ok_or_else(|| format!("PyPI release {name} {version} has no urls"))?;
    let sdist = urls.iter().find(|url| {
        url.get("packagetype")
            .and_then(|kind| kind.as_str())
            .is_some_and(|kind| kind == "sdist")
            && url
                .get("filename")
                .and_then(|name| name.as_str())
                .is_some_and(|name| name.ends_with(".tar.gz"))
    });
    let sdist = sdist.or_else(|| {
        urls.iter().find(|url| {
            url.get("packagetype")
                .and_then(|kind| kind.as_str())
                .is_some_and(|kind| kind == "sdist")
        })
    });
    let Some(sdist) = sdist else {
        return Err(format!("PyPI release {name} {version} has no sdist"));
    };
    let url = sdist
        .get("url")
        .and_then(|url| url.as_str())
        .ok_or_else(|| format!("PyPI sdist of {name} {version} has no url"))?;
    let bytes = client.get(url).map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err(format!("{url}: empty body"));
    }
    Ok(bytes)
}

fn download_cran_tarball(
    client: &dyn RegistryClient,
    name: &str,
    version: &str,
    _recipe: &ResolvedEasyconfig,
) -> Result<Vec<u8>, String> {
    let urls = [
        format!("https://cran.r-project.org/src/contrib/{name}_{version}.tar.gz"),
        format!("https://cran.r-project.org/src/contrib/Archive/{name}/{name}_{version}.tar.gz"),
    ];
    let mut errors = Vec::new();
    for url in urls {
        match client.get(&url) {
            Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
            Ok(_) => errors.push(format!("{url}: empty body")),
            Err(error) => errors.push(error.to_string()),
        }
    }
    Err(errors.join("; "))
}

fn expand_ext_template(template: &str, name: &str, version: &str) -> String {
    let letter = name.chars().next().unwrap_or('x').to_string();
    template
        .replace("%(version)s", version)
        .replace("%(namelower)s", &name.to_ascii_lowercase())
        .replace("%(nameletterlower)s", &letter.to_ascii_lowercase())
        .replace("%(nameletter)s", &letter)
        .replace("%(name)s", name)
}

fn join_url(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn parse_ext_tuple(
    raw: &str,
    recipe: &ResolvedEasyconfig,
    old_recipe_version: Option<&str>,
) -> Option<ExtDraft> {
    let trimmed = raw.trim().trim_end_matches(',').trim();
    if !trimmed.starts_with('(') {
        return None;
    }
    let (name_expr, version_expr) = tuple_head(trimmed)?;
    let (name, name_is_recipe) = interpret_expr(&name_expr, recipe, true)?;
    let (version, version_is_recipe) = interpret_expr(&version_expr, recipe, false)?;
    // `(name, version)` and a literal copy of the package's own version move
    // with the bump. A dependency that merely shares the same version number
    // does not: that pin is the author's, and today's newest release is not
    // what this generation shipped.
    let own_package = name == recipe.name;
    let tracks_recipe = (name_is_recipe && version_is_recipe)
        || (own_package
            && (version_is_recipe
                || old_recipe_version.is_some_and(|old| version == old)
                || version == recipe.version));
    let source_urls = quoted_list_after(trimmed, "source_urls");
    let mut source_templates = quoted_list_after(trimmed, "sources");
    if let Some(template) = quoted_value_after(trimmed, "source_tmpl") {
        source_templates.push(template);
    }
    Some(ExtDraft {
        raw: raw.trim().trim_end_matches(',').trim().to_string(),
        name,
        version,
        tracks_recipe,
        source_urls,
        source_templates,
    })
}

fn interpret_expr(
    expr: &str,
    recipe: &ResolvedEasyconfig,
    is_name: bool,
) -> Option<(String, bool)> {
    let expr = expr.trim();
    if let Some(literal) = unquote(expr) {
        return Some((literal, false));
    }
    if is_name && expr == "name" {
        return Some((recipe.name.clone(), true));
    }
    if !is_name && expr == "version" {
        return Some((recipe.version.clone(), true));
    }
    None
}

fn unquote(expr: &str) -> Option<String> {
    let bytes = expr.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let quote = bytes[0];
    if (quote != b'\'' && quote != b'"') || bytes[bytes.len() - 1] != quote {
        return None;
    }
    Some(expr[1..expr.len() - 1].to_string())
}

fn tuple_head(raw: &str) -> Option<(String, String)> {
    let bytes = raw.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut parts = Vec::new();
    let mut start = 1usize;
    for (index, &byte) in bytes.iter().enumerate().skip(1) {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b',' if depth == 0 => {
                parts.push(raw[start..index].trim().to_string());
                start = index + 1;
                if parts.len() == 2 {
                    break;
                }
            }
            b')' if depth == 0 => {
                parts.push(raw[start..index].trim().to_string());
                break;
            }
            _ => {}
        }
    }
    if parts.len() < 2 {
        return None;
    }
    Some((parts[0].clone(), parts[1].clone()))
}

fn quoted_value_after(raw: &str, key: &str) -> Option<String> {
    let pattern = format!(r#"['"]{key}['"]\s*:\s*['"]([^'"]*)['"]"#);
    let pattern = regex::Regex::new(&pattern).ok()?;
    pattern
        .captures(raw)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn quoted_list_after(raw: &str, key: &str) -> Vec<String> {
    let pattern = format!(r#"['"]{key}['"]\s*:\s*\[(.*?)\]"#);
    let Ok(pattern) = regex::Regex::new(&pattern) else {
        return Vec::new();
    };
    let Some(caps) = pattern.captures(raw) else {
        return Vec::new();
    };
    let body = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let quotes = regex::Regex::new(r#"['"]([^'"]*)['"]"#).ok();
    let Some(quotes) = quotes else {
        return Vec::new();
    };
    quotes
        .captures_iter(body)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn replace_version_literal(raw: &str, old: &str, new: &str) -> String {
    for quote in ['\'', '"'] {
        let from = format!("{quote}{old}{quote}");
        let to = format!("{quote}{new}{quote}");
        if let Some(index) = raw.find(&from) {
            let mut out = String::with_capacity(raw.len() + new.len());
            out.push_str(&raw[..index]);
            out.push_str(&to);
            out.push_str(&raw[index + from.len()..]);
            return out;
        }
    }
    raw.to_string()
}

fn replace_extension_checksum(
    raw: &str,
    old_version: &str,
    new_version: &str,
    digest: &str,
) -> String {
    // A patch file keeps the name it ships under, even when that name
    // embeds the old version; every other literal moves to the new one.
    let text = if !old_version.is_empty() && old_version != new_version {
        rename_non_patch_literals(raw, old_version, new_version)
    } else {
        raw.to_string()
    };
    // The archive checksum is the first entry of the extension's own
    // `checksums` list that is not keyed to, or commented as, a patch:
    // EasyBuild lists source checksums before patch checksums. The bump may
    // have emptied the archive entry, so the first literal digest left can
    // be a patch's; position, not the first 64-hex literal, decides.
    let Some((body_start, body_end)) = dict_list_span(&text, "checksums") else {
        return text;
    };
    let body = &text[body_start..body_end];
    for (start, end) in top_level_spans(body) {
        let entry = &body[start..end];
        let own_start = leading_comment_len(entry);
        let own = &entry[own_start..];
        let trailing = body[end..]
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches(',');
        if names_patch(own) || names_patch(trailing) {
            continue;
        }
        let Some((value_start, value_end)) = last_quoted_span(own) else {
            return text;
        };
        let absolute = body_start + start + own_start;
        let mut out = String::with_capacity(text.len() + digest.len());
        out.push_str(&text[..absolute + value_start]);
        out.push_str(digest);
        out.push_str(&text[absolute + value_end..]);
        return out;
    }
    text
}

/// Whether text names a patch file: a `.patch` or `.diff` key, or a
/// trailing comment naming one, as positional checksum lists write it.
fn names_patch(text: &str) -> bool {
    text.contains(".patch") || text.contains(".diff")
}

/// Length of the comment and blank lines that open a list entry span: the
/// comment trailing the previous entry lands there when the list is split.
pub(crate) fn leading_comment_len(entry: &str) -> usize {
    let mut offset = 0usize;
    for line in entry.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            offset += line.len();
        } else {
            let lead = line.len() - line.trim_start().len();
            return offset + lead;
        }
    }
    offset.min(entry.len())
}

/// Replace `old` with `new` inside every quoted literal and comment that
/// does not name a patch file.
fn rename_non_patch_literals(raw: &str, old: &str, new: &str) -> String {
    let Ok(pattern) = regex::Regex::new(r#"'[^'\n]*'|"[^"\n]*"|#[^\n]*"#) else {
        return raw.to_string();
    };
    pattern
        .replace_all(raw, |caps: &regex::Captures| {
            let literal = &caps[0];
            if names_patch(literal) {
                literal.to_string()
            } else {
                literal.replace(old, new)
            }
        })
        .into_owned()
}

/// Byte span of the contents of the last quoted string in `text`, ignoring
/// anything after a `#` comment.
fn last_quoted_span(text: &str) -> Option<(usize, usize)> {
    let mut quote: Option<(u8, usize)> = None;
    let mut found = None;
    let mut escaped = false;
    for (index, &byte) in text.as_bytes().iter().enumerate() {
        match quote {
            Some((active, open)) => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == active {
                    found = Some((open + 1, index));
                    quote = None;
                }
            }
            None => match byte {
                b'#' => break,
                b'\'' | b'"' => quote = Some((byte, index)),
                _ => {}
            },
        }
    }
    found
}

/// Body span, between the brackets, of the list a dict key names inside an
/// extension tuple: `'checksums': [ ... ]`.
fn dict_list_span(raw: &str, key: &str) -> Option<(usize, usize)> {
    let pattern = regex::Regex::new(&format!(r#"['"]{key}['"]\s*:\s*\["#)).ok()?;
    let open = pattern.find(raw)?.end();
    let mut depth = 1i32;
    let mut scan = Scanner::default();
    for (index, &byte) in raw.as_bytes().iter().enumerate().skip(open) {
        if !scan.code(byte) {
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

/// Trimmed byte spans of the top-level comma-separated entries of a list
/// body, split the way [`split_top_level`] splits it.
pub(crate) fn top_level_spans(body: &str) -> Vec<(usize, usize)> {
    fn push(body: &str, from: usize, to: usize, out: &mut Vec<(usize, usize)>) {
        let piece = &body[from..to];
        let lead = piece.len() - piece.trim_start().len();
        let trail = piece.len() - piece.trim_end().len();
        if lead + trail < piece.len() {
            out.push((from + lead, to - trail));
        }
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut scan = Scanner::default();
    for (index, &byte) in body.as_bytes().iter().enumerate() {
        if !scan.code(byte) {
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b',' if depth == 0 => {
                push(body, start, index, &mut out);
                start = index + 1;
            }
            _ => {}
        }
    }
    push(body, start, body.len(), &mut out);
    out
}

/// Byte-wise Python lexer state: quotes and `#` comments.
#[derive(Default)]
struct Scanner {
    quote: Option<u8>,
    escaped: bool,
    comment: bool,
}

impl Scanner {
    /// Feed one byte; true when it is code outside a string or comment.
    fn code(&mut self, byte: u8) -> bool {
        if self.comment {
            if byte == b'\n' {
                self.comment = false;
            }
            return false;
        }
        if let Some(active) = self.quote {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == active {
                self.quote = None;
            }
            return false;
        }
        match byte {
            b'#' => {
                self.comment = true;
                false
            }
            b'\'' | b'"' => {
                self.quote = Some(byte);
                false
            }
            _ => true,
        }
    }
}

fn assignment_text(text: &str, key: &str) -> Option<String> {
    let pattern = format!(r"(?m)^{key}\s*=");
    let pattern = regex::Regex::new(&pattern).ok()?;
    let start = pattern.find(text)?.start();
    let rest = &text[start..];
    let top = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*\s*(\+?=)").ok()?;
    let mut end = rest.len();
    for (index, line) in rest.lines().enumerate().skip(1) {
        if top.is_match(line) {
            end = rest.lines().take(index).map(|line| line.len() + 1).sum();
            break;
        }
    }
    Some(rest[..end].to_string())
}

fn list_body(text: &str, key: &str) -> Option<String> {
    let (start, end) = easyconfig_list_span(text, key).ok().flatten()?;
    Some(text[start..end].to_string())
}

fn replace_list_body(text: &str, key: &str, body: &str) -> Option<String> {
    let (start, end) = easyconfig_list_span(text, key).ok().flatten()?;
    let mut out = String::with_capacity(text.len() + body.len());
    out.push_str(&text[..start]);
    out.push_str(body);
    out.push_str(&text[end..]);
    Some(out)
}

fn split_top_level(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut comment = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'#' => comment = true,
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b',' if depth == 0 => {
                let piece = body[start..index].trim();
                if !piece.is_empty() {
                    out.push(piece.to_string());
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = body[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn render_list_body(entries: &[String]) -> String {
    let mut out = String::from("\n");
    for entry in entries {
        let entry = entry.trim().trim_end_matches(',').trim();
        if entry.is_empty() {
            continue;
        }
        let mut lines = entry.lines();
        if let Some(first) = lines.next() {
            out.push_str("    ");
            out.push_str(first.trim());
        }
        for line in lines {
            out.push('\n');
            out.push_str(line);
        }
        out.push_str(",\n");
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn residual(
    id: &str,
    category: &str,
    severity: ResidualSeverity,
    summary: &str,
    evidence: Option<String>,
) -> Residual {
    Residual {
        id: id.to_string(),
        stage: ResidualStage::Emit,
        category: category.to_string(),
        severity,
        summary: summary.to_string(),
        evidence,
        provenance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MapClient;

    fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            for (path, content) in files {
                let data = content.as_bytes();
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, *path, data).unwrap();
            }
            builder.finish().unwrap();
        }
        encoder.finish().unwrap()
    }

    #[test]
    fn cargo_lock_replaces_the_crates_list_and_their_checksums() {
        let lock = r#"
version = 3

[[package]]
name = "demo"
version = "2.0.0"

[[package]]
name = "leftpad"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[package]]
name = "rightpad"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#;
        let manifest = "[package]\nname = \"demo\"\nversion = \"2.0.0\"\n";
        let archive = tar_gz(&[
            ("demo-2.0.0/Cargo.toml", manifest),
            ("demo-2.0.0/Cargo.lock", lock),
        ]);
        let text = "\
easyblock = 'Cargo'
name = 'demo'
version = '2.0.0'
homepage = 'https://example.invalid'
description = 'x'
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
sources = ['v%(version)s.tar.gz']
source_urls = ['https://example.invalid/tags']
crates = [
    ('leftpad', '0.1.0'),
]
checksums = [
    {'v2.0.0.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'},
    {'leftpad-0.1.0.tar.gz': 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'},
]
moduleclass = 'tools'
";
        let archives = [FetchedArchive {
            filename: "v2.0.0.tar.gz",
            bytes: &archive,
        }];
        let (rewritten, residuals) =
            rewrite_fetched_recipe(text, None, &archives, &MapClient::default());
        assert!(residuals.is_empty(), "{residuals:?}");
        assert!(rewritten.contains("('leftpad', '1.0.0')"), "{rewritten}");
        assert!(rewritten.contains("('rightpad', '0.2.0')"), "{rewritten}");
        assert!(!rewritten.contains("0.1.0"), "{rewritten}");
        assert!(
            rewritten.contains(
                "{'leftpad-1.0.0.tar.gz': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'}"
            ),
            "{rewritten}"
        );
        assert!(
            rewritten.contains(
                "{'v2.0.0.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'}"
            ),
            "{rewritten}"
        );
        assert!(
            rewritten.contains(
                "{'rightpad-0.2.0.tar.gz': 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'}"
            ),
            "{rewritten}"
        );
    }

    fn bundle(version: &str, python: &str, ext_version: &str, checksum: &str) -> String {
        format!(
            "\
easyblock = 'PythonBundle'
name = 'Widget'
version = '{version}'
homepage = 'https://example.invalid'
description = 'x'
toolchain = {{'name': 'GCCcore', 'version': '14.3.0'}}
dependencies = [('Python', '{python}')]
exts_list = [
    (name, version, {{
        'checksums': ['{checksum}'],
    }}),
    ('other', '{ext_version}', {{
        'checksums': ['eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'],
    }}),
]
moduleclass = 'lib'
"
        )
    }

    #[test]
    fn an_extension_that_is_the_package_gets_the_new_sdist_checksum() {
        let prior = crate::eb_parse::resolve_easyconfig_str(&bundle(
            "1.0.0",
            "3.11.5",
            "1.0.0",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))
        .unwrap();
        let text = bundle(
            "2.0.0",
            "3.11.5",
            "1.0.0",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let mut client = MapClient::default();
        client.pages.insert(
            "https://pypi.org/pypi/Widget/2.0.0/json".into(),
            br#"{
                "info": {"name": "Widget", "version": "2.0.0", "requires_python": ">=3.8"},
                "urls": [{
                    "packagetype": "sdist",
                    "filename": "Widget-2.0.0.tar.gz",
                    "url": "https://files.example/Widget-2.0.0.tar.gz"
                }]
            }"#
            .to_vec(),
        );
        client.pages.insert(
            "https://files.example/Widget-2.0.0.tar.gz".into(),
            b"sdist".to_vec(),
        );
        let (rewritten, residuals) = rewrite_fetched_recipe(&text, Some(&prior), &[], &client);
        assert!(residuals.is_empty(), "{residuals:?}");
        let digest = sha256_hex(b"sdist");
        assert!(rewritten.contains(&digest), "{rewritten}");
        assert!(
            rewritten
                .contains("'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'"),
            "{rewritten}"
        );
        assert!(rewritten.contains("('other', '1.0.0'"), "{rewritten}");
    }

    #[test]
    fn an_emptied_archive_checksum_is_filled_and_patch_digests_stay() {
        let raw = "(name, version, {
        'patches': [
            ('Widget-0.3.2_avx.patch', 1),
            {'name': 'Widget-1.0.0_fix.patch', 'opts': '--binary'},
        ],
        'checksums': [
            {'Widget-2.0.0.tar.gz': ''},
            {'Widget-0.3.2_avx.patch': '1111111111111111111111111111111111111111111111111111111111111111'},
            {'Widget-1.0.0_fix.patch':
             '2222222222222222222222222222222222222222222222222222222222222222'},
        ],
    })";
        let digest = "3333333333333333333333333333333333333333333333333333333333333333";
        let out = replace_extension_checksum(raw, "1.0.0", "2.0.0", digest);
        assert!(
            out.contains(&format!("{{'Widget-2.0.0.tar.gz': '{digest}'}}")),
            "{out}"
        );
        assert!(out.contains("'1111111111111111111111111111111111111111111111111111111111111111'"));
        assert!(out.contains("'2222222222222222222222222222222222222222222222222222222222222222'"));
        assert!(out.contains("'Widget-1.0.0_fix.patch'"), "{out}");
    }

    #[test]
    fn a_positional_archive_checksum_ahead_of_commented_patches_is_replaced() {
        let raw = "('other', '1.0.0', {
        'patches': ['other-1.0.0_fix.patch'],
        'checksums': [
            'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',  # other-1.0.0.tar.gz
            'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',  # other-1.0.0_fix.patch
        ],
    })";
        let digest = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let out = replace_extension_checksum(raw, "1.0.0", "2.0.0", digest);
        assert!(
            out.contains(&format!("'{digest}',  # other-2.0.0.tar.gz")),
            "{out}"
        );
        assert!(
            out.contains(
                "'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',  # other-1.0.0_fix.patch"
            ),
            "{out}"
        );
        assert!(out.contains("['other-1.0.0_fix.patch']"), "{out}");
    }

    #[test]
    fn an_extension_the_new_python_cannot_import_moves_to_the_newest_sdist() {
        let prior = crate::eb_parse::resolve_easyconfig_str(&bundle(
            "1.0.0",
            "3.10.4",
            "1.0.0",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ))
        .unwrap();
        let text = bundle(
            "1.0.0",
            "3.12.0",
            "1.0.0",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let mut client = MapClient::default();
        client.pages.insert(
            "https://pypi.org/pypi/other/1.0.0/json".into(),
            br#"{"info": {"version": "1.0.0", "requires_python": "<3.11"}, "urls": []}"#.to_vec(),
        );
        client.pages.insert(
            "https://pypi.org/pypi/other/json".into(),
            br#"{
                "info": {"version": "2.0.0", "requires_python": ">=3.11"},
                "releases": {
                    "1.0.0": [{"packagetype": "sdist", "filename": "other-1.0.0.tar.gz", "requires_python": "<3.11", "yanked": false}],
                    "2.0.0": [{"packagetype": "sdist", "filename": "other-2.0.0.tar.gz", "requires_python": ">=3.11", "yanked": false}],
                    "2.1.0a1": [{"packagetype": "sdist", "filename": "other-2.1.0a1.tar.gz", "requires_python": ">=3.11", "yanked": false}]
                }
            }"#
            .to_vec(),
        );
        client.pages.insert(
            "https://pypi.org/pypi/other/2.0.0/json".into(),
            br#"{
                "info": {"version": "2.0.0", "requires_python": ">=3.11"},
                "urls": [{"packagetype": "sdist", "filename": "other-2.0.0.tar.gz", "url": "https://files.example/other-2.0.0.tar.gz"}]
            }"#
            .to_vec(),
        );
        client.pages.insert(
            "https://files.example/other-2.0.0.tar.gz".into(),
            b"new".to_vec(),
        );
        // The package extension itself also changed only if versions differ.
        // Widget stayed at 1.0.0, so its checksum is not fetched.
        let (rewritten, residuals) = rewrite_fetched_recipe(&text, Some(&prior), &[], &client);
        assert!(residuals.is_empty(), "{residuals:?}\n{rewritten}");
        assert!(rewritten.contains("('other', '2.0.0'"), "{rewritten}");
        assert!(!rewritten.contains("2.1.0a1"), "{rewritten}");
        assert!(rewritten.contains(&sha256_hex(b"new")), "{rewritten}");
    }

    #[test]
    fn a_git_crate_is_listed_without_a_lock_checksum() {
        let lock = r#"
version = 3

[[package]]
name = "demo"
version = "2.0.0"

[[package]]
name = "leftpad"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[package]]
name = "forked"
version = "0.1.0"
source = "git+https://github.com/example/forked?rev=abcdef1234567890#abcdef1234567890"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#;
        let manifest = "[package]\nname = \"demo\"\nversion = \"2.0.0\"\n";
        let archive = tar_gz(&[
            ("demo-2.0.0/Cargo.toml", manifest),
            ("demo-2.0.0/Cargo.lock", lock),
        ]);
        let text = "\
easyblock = 'Cargo'
name = 'demo'
version = '2.0.0'
homepage = 'https://example.invalid'
description = 'x'
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
sources = ['v%(version)s.tar.gz']
source_urls = ['https://example.invalid/tags']
crates = [
    ('leftpad', '0.9.0'),
    (
        'forked', '0.1.0',
        'https://github.com/example/forked',
        'abcdef1234567890',
    ),
]
checksums = [
    {'v2.0.0.tar.gz': 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'},
    {'leftpad-0.9.0.tar.gz': 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'},
    {'forked-0.1.0-abcdef12.tar.xz': 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'},
]
moduleclass = 'tools'
";
        let archives = [FetchedArchive {
            filename: "v2.0.0.tar.gz",
            bytes: &archive,
        }];
        let (rewritten, residuals) =
            rewrite_fetched_recipe(text, None, &archives, &MapClient::default());
        assert!(
            rewritten.contains(
                "('forked', '0.1.0', 'https://github.com/example/forked', 'abcdef1234567890')"
            ),
            "{rewritten}"
        );
        assert!(!rewritten.contains("forked-0.1.0"), "{rewritten}");
        assert!(
            residuals.iter().any(|item| item.id == "crates:git"),
            "{residuals:?}"
        );
    }
}
