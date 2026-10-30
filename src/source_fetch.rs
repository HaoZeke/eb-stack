//! Fill the source checksum a version bump invalidates, the way
//! `eb --inject-checksums` does: download each `sources` entry from the
//! recipe's own `source_urls`, in order, and hash the bytes.
//!
//! A bump that changes the version clears the old digest because it is not a
//! claim about the new tarball. EasyBuild's answer is `--inject-checksums`,
//! which fetches the artifact the recipe names and writes its SHA-256. This
//! module does the same before emission, so the bumped recipe carries a real
//! digest and the SBOM's source component carries the same hash. Nothing here
//! invents a URL. An entry with no URL, a git checkout, or a failed download
//! stays unresolved and is reported as a residual with the reason, never
//! filled. The one URL EasyBuild itself supplies is kept: `PythonPackage`
//! (and an easyblock that takes its `extra_options`, such as
//! `CargoPythonPackage` or `PythonBundle`) defaults `source_urls` to
//! `[PYPI_SOURCE]` when the recipe leaves the list unset. That default is
//! used only for the download. It is not written back into the recipe.

use crate::eb_parse::{resolve_easyconfig_str, ResolvedEasyconfig, SourceFile};
use crate::package::{Residual, ResidualSeverity, ResidualStage};
use crate::package_workflow::{
    plan_package_bump_with, BumpPackageRequest, PackageBundle, PackageEmitOptions,
    PackageWorkflowError,
};
use crate::registry::RegistryClient;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// What happened to one `sources` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFetch {
    /// Name the artifact is saved under.
    pub filename: String,
    /// URL that served it, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// SHA-256 of the downloaded bytes, when the download succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Every URL tried and why it failed, or why none could be tried.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// Body of a successful download. Not part of `sources.fetch.json`; the
    /// index rewrite reads `Cargo.lock` out of it and then drops it.
    #[serde(skip)]
    pub bytes: Option<Vec<u8>>,
    /// The recipe's download name and the one that served the archive, when
    /// the release tag is spelled the other way: a project that moved from
    /// `1.0.8` to `v1.0.10` tags serves `/archive/v1.0.10.tar.gz` and 404s on
    /// `/archive/1.0.10.tar.gz`. The recipe's `sources` entry is rewritten to
    /// the name that served it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respelled: Option<Respelling>,
}

/// A source download name the archive host served under the other spelling
/// of the release tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Respelling {
    /// Name the recipe asked for, which the host does not serve.
    pub from: String,
    /// Name that served the archive.
    pub to: String,
}

/// The other spelling of a release tag in an archive download name: a
/// leading `v` added or removed. Only archive endpoints, which name the
/// tarball after the tag, and only names that start with the version.
fn other_tag_spelling(bases: &[String], download: &str, version: &str) -> Option<String> {
    if version.is_empty() || !bases.iter().any(|base| base.contains("/archive")) {
        return None;
    }
    if download.starts_with(version) {
        return Some(format!("v{download}"));
    }
    download
        .strip_prefix('v')
        .filter(|rest| rest.starts_with(version))
        .map(str::to_string)
}

/// The URLs EasyBuild would try for `file`, in order: the entry's own
/// `source_urls` when the dict sets them, else the recipe's, else the
/// easyblock default from [`pypi_default_bases`].
pub fn source_download_urls(recipe: &ResolvedEasyconfig, file: &SourceFile) -> Vec<String> {
    let inherited = if file.source_urls.is_empty() && recipe.source_urls.is_empty() {
        pypi_default_bases(recipe)
    } else {
        None
    };
    let bases: &[String] = if !file.source_urls.is_empty() {
        &file.source_urls
    } else if !recipe.source_urls.is_empty() {
        &recipe.source_urls
    } else if let Some(bases) = inherited.as_ref() {
        bases
    } else {
        &[]
    };
    bases
        .iter()
        .filter(|base| !base.contains("%("))
        .map(|base| {
            if base.ends_with('/') {
                format!("{base}{}", file.download_filename)
            } else {
                format!("{base}/{}", file.download_filename)
            }
        })
        .collect()
}

/// `PythonPackage.extra_options` sets `source_urls` to `[PYPI_SOURCE]` when
/// the recipe did not. `CargoPythonPackage` calls that first, so a name
/// ending in `PythonPackage` inherits the same default. `PythonBundle`
/// calls it too, after `Bundle.extra_options`, which does not set
/// `source_urls` itself.
fn pypi_default_bases(recipe: &ResolvedEasyconfig) -> Option<Vec<String>> {
    let block = recipe.easyblock.as_deref()?;
    if !(block.ends_with("PythonPackage") || block == "PythonBundle") {
        return None;
    }
    pypi_source_base(&recipe.name).map(|base| vec![base])
}

/// `PYPI_SOURCE` with `%(nameletter)s` and `%(name)s` filled. The letter is
/// the first character of the name, not its lowercase form: that is the
/// template EasyBuild resolves, and PyPI ignores casing in the path.
fn pypi_source_base(name: &str) -> Option<String> {
    let pattern = crate::eb_template_constants::EB_TEMPLATE_CONSTANTS
        .iter()
        .find(|(key, _)| *key == "PYPI_SOURCE")
        .map(|(_, value)| *value)?;
    let letter = name.chars().next()?;
    Some(
        pattern
            .replace("%(nameletter)s", &letter.to_string())
            .replace("%(name)s", name),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

/// Download and hash every `sources` entry of `recipe`.
pub fn fetch_sources(recipe: &ResolvedEasyconfig, client: &dyn RegistryClient) -> Vec<SourceFetch> {
    recipe
        .source_files
        .iter()
        .map(|file| {
            let mut fetch = SourceFetch {
                filename: file.filename.clone(),
                url: None,
                sha256: None,
                errors: Vec::new(),
                bytes: None,
                respelled: None,
            };
            if file.git {
                fetch.errors.push(
                    "git_config source: the tarball is made from a checkout at build time".into(),
                );
                return fetch;
            }
            if file.filename.contains("%(") || file.download_filename.contains("%(") {
                fetch
                    .errors
                    .push("filename keeps an unresolved template".into());
                return fetch;
            }
            let urls = source_download_urls(recipe, file);
            if urls.is_empty() {
                fetch
                    .errors
                    .push("no source_urls entry to download from".into());
            }
            for url in urls {
                match client.get(&url) {
                    Ok(bytes) if !bytes.is_empty() => {
                        fetch.sha256 = Some(sha256_hex(&bytes));
                        fetch.url = Some(url);
                        fetch.bytes = Some(bytes);
                        break;
                    }
                    Ok(_) => fetch.errors.push(format!("{url}: empty body")),
                    Err(error) => fetch.errors.push(error.to_string()),
                }
            }
            if fetch.sha256.is_none() {
                try_other_tag_spelling(recipe, file, client, &mut fetch);
            }
            fetch
        })
        .collect()
}

/// Retry an archive download under the other spelling of the release tag.
/// The errors of the recipe's own spelling stay in the record.
fn try_other_tag_spelling(
    recipe: &ResolvedEasyconfig,
    file: &SourceFile,
    client: &dyn RegistryClient,
    fetch: &mut SourceFetch,
) {
    let bases: Vec<String> = if !file.source_urls.is_empty() {
        file.source_urls.clone()
    } else {
        recipe.source_urls.clone()
    };
    let Some(alternate) = other_tag_spelling(&bases, &file.download_filename, &recipe.version)
    else {
        return;
    };
    let respelled_file = SourceFile {
        download_filename: alternate.clone(),
        ..file.clone()
    };
    for url in source_download_urls(recipe, &respelled_file) {
        match client.get(&url) {
            Ok(bytes) if !bytes.is_empty() => {
                fetch.sha256 = Some(sha256_hex(&bytes));
                fetch.url = Some(url);
                fetch.bytes = Some(bytes);
                fetch.respelled = Some(Respelling {
                    from: file.download_filename.clone(),
                    to: alternate.clone(),
                });
                // A plain string entry saves the archive under the name it
                // downloads; a dict entry keeps its own `filename`.
                if file.filename == file.download_filename {
                    fetch.filename = alternate;
                }
                fetch.errors.clear();
                return;
            }
            Ok(_) => fetch.errors.push(format!("{url}: empty body")),
            Err(error) => fetch.errors.push(error.to_string()),
        }
    }
}

/// Rewrite the `sources` entry, and a checksum keyed by its saved name, to
/// the tag spelling the archive host served.
fn apply_respellings(bundle: &mut PackageBundle, fetched: &[SourceFetch]) {
    let Some(easyconfig) = bundle.easyconfigs.first_mut() else {
        return;
    };
    let Ok(recipe) = resolve_easyconfig_str(&easyconfig.text) else {
        return;
    };
    let version = recipe.version.clone();
    for fetch in fetched {
        let Some(respelling) = &fetch.respelled else {
            continue;
        };
        let adding = respelling.to.len() > respelling.from.len();
        for key in ["sources", "checksums"] {
            let Ok(Some((start, end))) =
                crate::eb_emit::easyconfig_list_span(&easyconfig.text, key)
            else {
                continue;
            };
            let body = &easyconfig.text[start..end];
            let rewritten = respell_literals(body, &version, &respelling.from, adding);
            if rewritten != body {
                easyconfig.text = format!(
                    "{}{}{}",
                    &easyconfig.text[..start],
                    rewritten,
                    &easyconfig.text[end..]
                );
            }
        }
    }
}

/// Add or drop the leading `v` of every quoted literal that resolves to
/// `from` once `%(version)s` is filled.
fn respell_literals(body: &str, version: &str, from: &str, adding: bool) -> String {
    let Ok(literal) = regex::Regex::new(r#"(['"])([^'"\n]*)(['"])"#) else {
        return body.to_string();
    };
    literal
        .replace_all(body, |caps: &regex::Captures| {
            let inner = &caps[2];
            if inner.replace("%(version)s", version) != from {
                return caps[0].to_string();
            }
            let respelled = if adding {
                format!("v{inner}")
            } else {
                inner.strip_prefix('v').unwrap_or(inner).to_string()
            };
            format!("{}{respelled}{}", &caps[1], &caps[3])
        })
        .into_owned()
}

/// Drop the "checksum cleared" review warnings once every checksum in the
/// final recipe, top level and extensions alike, carries a digest.
///
/// The emitter warns while it clears a stale digest; the fetch then fills
/// the entry. A warning about an entry that is no longer empty describes a
/// recipe that was never written out.
fn prune_settled_checksum_warnings(bundle: &mut PackageBundle) {
    let Some(easyconfig) = bundle.easyconfigs.first() else {
        return;
    };
    if has_empty_checksum(&easyconfig.text) {
        return;
    }
    bundle.plan.residuals.retain(|residual| {
        !(residual.category == "bump-warning"
            && (residual.summary.contains("source checksum cleared")
                || residual.summary.contains("stale digest cleared")))
    });
}

/// Whether any `checksums` list in `text`, top level or inside an
/// extension's options, holds an empty string.
fn has_empty_checksum(text: &str) -> bool {
    let Ok(header) = regex::Regex::new(r#"checksums['"]?\s*[:=]\s*\["#) else {
        return true;
    };
    let Ok(empty) = regex::Regex::new(r#"(^|[\[{:,\s])(''|"")"#) else {
        return true;
    };
    for found in header.find_iter(text) {
        let open = found.end();
        let mut depth = 1i32;
        let mut close = text.len();
        for (offset, byte) in text.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        if empty.is_match(&text[open..close]) {
            return true;
        }
    }
    false
}

/// Plan a bump, then fill the source checksum the version change cleared.
///
/// The first pass emits the recipe at the new version, so its `sources` and
/// `source_urls` resolve with the new templates. The first source's digest is
/// then passed as `source_checksum` and the bump is planned again, which is
/// the existing, tested path for a stated checksum. Entries that could not be
/// fetched, and sources after the first, become `source:fetch` residuals; the
/// bump never invents a digest. A request that already states a checksum, or
/// keeps the version, is planned once and unchanged.
///
/// With the new source in hand, every patch the tree could not decide is
/// tried against it (see [`crate::patch_apply`]): a patch that applies is
/// carried, one the release already contains or whose files are gone is
/// dropped from the recipe, and only a patch that does neither stays
/// undecided. `strict_patches` fails on those that remain.
pub fn plan_package_bump_fetching(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: &dyn RegistryClient,
) -> Result<(PackageBundle, Vec<SourceFetch>), PackageWorkflowError> {
    plan_package_bump_fetching_with(request, options, client, &BumpDecisions::default())
}

/// What a fetching bump takes from its operator rather than from evidence.
#[derive(Debug, Clone, Default)]
pub struct BumpDecisions {
    /// Patches to drop, by file name, each with the reason the plan records.
    /// A name the recipe does not list fails the bump.
    pub drop_patches: Vec<(String, String)>,
    /// Move every later source whose version a `local_*` variable holds to
    /// its newest stable upstream release, read the way `package latest`
    /// reads the package's own.
    pub latest_secondary: bool,
    /// Upstream overrides for those sources, keyed by the component name
    /// that precedes the version in the file name (`libatomic_ops`).
    pub overrides: std::collections::BTreeMap<String, crate::upstream::Override>,
}

/// [`plan_package_bump_fetching`] with the operator's decisions.
///
/// A patch the source leaves undecided is replaced by a rebased file named
/// for the new release (`<Name>-<version>_<description>.patch`, or the old
/// name with its version replaced) when one sits beside the source recipe or
/// under `<letter>/<Name>` of an easyconfigs root and applies to the new
/// source. `decisions.drop_patches` then drop what the operator decided,
/// with the reason in the plan, and `decisions.latest_secondary` moves
/// version-pinned later sources (see [`bump_secondary_sources`]).
pub fn plan_package_bump_fetching_with(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: &dyn RegistryClient,
    decisions: &BumpDecisions,
) -> Result<(PackageBundle, Vec<SourceFetch>), PackageWorkflowError> {
    if request.version.is_none() || request.source_checksum.is_some() {
        let mut bundle = plan_package_bump_with(request, options)?;
        let mut fetched = Vec::new();
        if decisions.latest_secondary {
            bump_secondary_sources(&mut bundle, client, &decisions.overrides, &mut fetched);
            crate::package_workflow::refresh_bump_sbom(&mut bundle)?;
        }
        if !decisions.drop_patches.is_empty() {
            let names: Vec<&str> = decisions
                .drop_patches
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            return Err(PackageWorkflowError::UnknownPatchDrop(format!(
                "{} (patch drops are decided against a fetched new release)",
                names.join(", ")
            )));
        }
        return Ok((bundle, fetched));
    }
    // Strict patch mode is judged after the new source has had its say: a
    // patch the tree cannot decide may still apply, or be upstream.
    let mut lenient = request.clone();
    lenient.strict_patches = false;
    let first = plan_package_bump_with(&lenient, options)?;
    let Some(emitted) = first.easyconfigs.first() else {
        return Ok((first, Vec::new()));
    };
    let recipe = resolve_easyconfig_str(&emitted.text)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    let mut fetched = fetch_sources(&recipe, client);
    let mut bundle = match fetched.first().and_then(|fetch| fetch.sha256.clone()) {
        Some(digest) => {
            let mut again = lenient.clone();
            again.source_checksum = Some(digest);
            let mut bundle = plan_package_bump_with(&again, options)?;
            apply_respellings(&mut bundle, &fetched);
            push_fetch_residuals(&mut bundle, &fetched, 1);
            bundle
        }
        None => {
            let mut bundle = first;
            push_fetch_residuals(&mut bundle, &fetched, 0);
            bundle
        }
    };
    fill_empty_named_checksums(&mut bundle, client, &mut fetched);
    if decisions.latest_secondary {
        bump_secondary_sources(&mut bundle, client, &decisions.overrides, &mut fetched);
    }
    apply_index_rewrites(&mut bundle, request, &fetched, client);
    prune_settled_checksum_warnings(&mut bundle);
    let new_version = request.version.as_deref().unwrap_or_default();
    let prior_version = std::fs::read_to_string(&request.source)
        .ok()
        .and_then(|text| resolve_easyconfig_str(&text).ok())
        .map(|recipe| recipe.version);
    let inputs = crate::patch_apply::PatchInputs {
        rebased_dirs: rebased_patch_dirs(request, &bundle.plan.package.name),
        drops: decisions.drop_patches.clone(),
    };
    let mut used_drops = Vec::new();
    crate::patch_apply::decide_extension_patches(
        &mut bundle,
        prior_version.as_deref(),
        client,
        &inputs,
        &mut used_drops,
    );
    let undecided = crate::patch_apply::decide_from_source(
        &mut bundle,
        new_version,
        &fetched,
        &inputs,
        &mut used_drops,
    );
    let unknown: Vec<&str> = decisions
        .drop_patches
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !used_drops.iter().any(|used| used == name))
        .collect();
    if !unknown.is_empty() {
        return Err(PackageWorkflowError::UnknownPatchDrop(unknown.join(", ")));
    }
    crate::package_workflow::refresh_bump_sbom(&mut bundle)?;
    if request.strict_patches && !undecided.is_empty() {
        return Err(PackageWorkflowError::UndecidedPatches(undecided.join(", ")));
    }
    Ok((bundle, fetched))
}

/// Where a patch rebased on the new release may sit: beside the source
/// recipe, then at `<letter>/<Name>` under each easyconfigs root, the
/// layout EasyBuild's robot path search expects.
fn rebased_patch_dirs(request: &BumpPackageRequest, name: &str) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(parent) = request.source.parent() {
        dirs.push(parent.to_path_buf());
    }
    let letter = crate::eb_parse::easyconfig_letter_dir(name);
    for root in &request.easyconfig_roots {
        let dir = root.join(&letter).join(name);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Move each later source whose version a `local_*` variable holds to its
/// newest stable upstream release.
///
/// A source after the first counts when its file name holds the value of a
/// version-valued `local_*` assignment and the `sources` list names that
/// variable: gc's `'libatomic_ops-%s.tar.gz' % local_libatomic_version`.
/// The component is the file name before the version (`libatomic_ops`), and
/// its releases are read with the backends of [`crate::upstream`] from the
/// `source_urls` that carry the version, keyed by the component in
/// `overrides`. A newer release rewrites the variable, downloads the new
/// file, and writes its digest where the old one was. Each source gets a
/// `secondary-source` record: the move, that it is current, or why no
/// version could be decided.
pub fn bump_secondary_sources(
    bundle: &mut PackageBundle,
    client: &dyn RegistryClient,
    overrides: &std::collections::BTreeMap<String, crate::upstream::Override>,
    fetched: &mut Vec<SourceFetch>,
) {
    let Some(text) = bundle.easyconfigs.first().map(|ec| ec.text.clone()) else {
        return;
    };
    let Ok(recipe) = resolve_easyconfig_str(&text) else {
        return;
    };
    let Ok(local) =
        regex::Regex::new(r#"(?m)^(local_[A-Za-z0-9_]+)[ \t]*=[ \t]*['"]([0-9][^'"\n]*)['"]"#)
    else {
        return;
    };
    let sources_text = crate::eb_emit::easyconfig_list_span(&text, "sources")
        .ok()
        .flatten()
        .map(|(start, end)| text[start..end].to_string())
        .unwrap_or_default();
    let locals: Vec<(String, String)> = local
        .captures_iter(&text)
        .map(|caps| (caps[1].to_string(), caps[2].to_string()))
        .filter(|(variable, _)| sources_text.contains(variable.as_str()))
        .collect();
    let mut text = text;
    for (index, file) in recipe.source_files.iter().enumerate().skip(1) {
        let Some((variable, current)) = locals
            .iter()
            .find(|(_, value)| file.filename.contains(value.as_str()))
        else {
            continue;
        };
        let at = file.filename.find(current.as_str()).unwrap_or(0);
        let component = file.filename[..at].trim_end_matches(['-', '_']).to_string();
        if component.is_empty() {
            continue;
        }
        let bases: Vec<String> = if file.source_urls.is_empty() {
            recipe.source_urls.clone()
        } else {
            file.source_urls.clone()
        };
        let own: Vec<String> = bases
            .iter()
            .filter(|base| base.contains(current.as_str()))
            .cloned()
            .collect();
        let mut synthetic = recipe.clone();
        synthetic.name = component.clone();
        synthetic.version = current.clone();
        synthetic.source_urls = if own.is_empty() { bases } else { own };
        synthetic.source_files = vec![file.clone()];
        synthetic.homepage = None;
        synthetic.easyblock = None;
        let mut record = |severity: ResidualSeverity, summary: String, evidence: Option<String>| {
            bundle.plan.residuals.push(Residual {
                id: format!("secondary-source:{index}"),
                stage: ResidualStage::Normalize,
                category: "secondary-source".into(),
                severity,
                summary,
                evidence,
                provenance: None,
            });
        };
        let report = match crate::upstream::latest(&synthetic, overrides, client) {
            Ok(report) => report,
            Err(error) => {
                record(
                    ResidualSeverity::Judgment,
                    format!("{component} {current}: no upstream version decided"),
                    Some(format!("{error:#}")),
                );
                continue;
            }
        };
        let newest = match crate::upstream::bump_version(&report) {
            Ok(Some(newest)) => newest,
            Ok(None) => {
                record(
                    ResidualSeverity::Mechanical,
                    format!(
                        "{component} {current} is the newest stable release on {}",
                        report.backend
                    ),
                    None,
                );
                continue;
            }
            Err(error) => {
                record(
                    ResidualSeverity::Judgment,
                    format!("{component} {current}: no upstream version decided"),
                    Some(format!("{error:#}")),
                );
                continue;
            }
        };
        if crate::version::cmp_version(&newest, current) != std::cmp::Ordering::Greater {
            continue;
        }
        let assignment = format!(
            r#"(?m)^({}[ \t]*=[ \t]*['"]){}(['"])"#,
            regex::escape(variable),
            regex::escape(current)
        );
        let Ok(assignment) = regex::Regex::new(&assignment) else {
            continue;
        };
        let moved = assignment
            .replace(&text, |caps: &regex::Captures| {
                format!("{}{newest}{}", &caps[1], &caps[2])
            })
            .into_owned();
        let Ok(moved_recipe) = resolve_easyconfig_str(&moved) else {
            continue;
        };
        let Some(new_file) = moved_recipe.source_files.get(index) else {
            continue;
        };
        let mut fetch = SourceFetch {
            filename: new_file.filename.clone(),
            url: None,
            sha256: None,
            errors: Vec::new(),
            bytes: None,
            respelled: None,
        };
        for url in source_download_urls(&moved_recipe, new_file) {
            match client.get(&url) {
                Ok(bytes) if !bytes.is_empty() => {
                    fetch.sha256 = Some(sha256_hex(&bytes));
                    fetch.url = Some(url);
                    fetch.bytes = Some(bytes);
                    break;
                }
                Ok(_) => fetch.errors.push(format!("{url}: empty body")),
                Err(error) => fetch.errors.push(error.to_string()),
            }
        }
        let Some(digest) = fetch.sha256.clone() else {
            record(
                ResidualSeverity::Blocking,
                format!(
                    "{component} {current} -> {newest}: the new source {} could not be downloaded",
                    new_file.filename
                ),
                Some(fetch.errors.join("; ")),
            );
            continue;
        };
        text = crate::patch_apply::replace_checksum(
            &moved,
            &recipe,
            &file.filename,
            &new_file.filename,
            &digest,
            index,
        );
        record(
            ResidualSeverity::Mechanical,
            format!(
                "{component} {current} -> {newest}: newest stable release on {}",
                report.backend
            ),
            Some(format!(
                "sha256 {digest} from {}",
                fetch.url.as_deref().unwrap_or("")
            )),
        );
        if let Some(source) = bundle.plan.sources.get_mut(index) {
            source.sha256 = Some(digest);
        }
        // What the fetch said about the old file no longer describes the
        // recipe.
        bundle.plan.residuals.retain(|residual| {
            !(residual.category == "checksum" && residual.summary.contains(&file.filename))
        });
        fetched.retain(|seen| seen.filename != file.filename);
        fetched.push(fetch);
    }
    if let Some(easyconfig) = bundle.easyconfigs.first_mut() {
        easyconfig.text = text;
    }
}

/// Versions, crate lists, and extension checksums an index can decide.
///
/// Runs after the version bump has been emitted, so `%(version)s` already
/// names the new release and a `Cargo.lock` inside the downloaded source is
/// the lock of that release. A toolchain-only bump never reaches here.
fn apply_index_rewrites(
    bundle: &mut PackageBundle,
    request: &BumpPackageRequest,
    fetched: &[SourceFetch],
    client: &dyn RegistryClient,
) {
    let Some(easyconfig) = bundle.easyconfigs.first() else {
        return;
    };
    let prior = std::fs::read_to_string(&request.source)
        .ok()
        .and_then(|text| resolve_easyconfig_str(&text).ok());
    let archives = fetched
        .iter()
        .filter_map(|fetch| {
            fetch
                .bytes
                .as_deref()
                .map(|bytes| crate::index_bump::FetchedArchive {
                    filename: fetch.filename.as_str(),
                    bytes,
                })
        })
        .collect::<Vec<_>>();
    let (text, residuals) = crate::index_bump::rewrite_fetched_recipe(
        &easyconfig.text,
        prior.as_ref(),
        &archives,
        client,
    );
    if let Some(easyconfig) = bundle.easyconfigs.first_mut() {
        easyconfig.text = text;
    }
    bundle.plan.residuals.extend(residuals);
}

/// Download every concrete checksum-dict filename whose digest is empty.
///
/// `rewrite_source_checksum` keeps one key per file and clears the ones the
/// version bump renamed. `--inject-checksums` then hashes each of those
/// files from the recipe's `source_urls`. Only an empty literal entry is
/// written; a digest that is already present, or a key that is still a
/// template, is left alone.
fn fill_empty_named_checksums(
    bundle: &mut PackageBundle,
    client: &dyn RegistryClient,
    fetched: &mut Vec<SourceFetch>,
) {
    let Some(original) = bundle
        .easyconfigs
        .first()
        .map(|easyconfig| easyconfig.text.clone())
    else {
        return;
    };
    let already: std::collections::HashSet<&str> = fetched
        .iter()
        .filter(|fetch| fetch.sha256.is_some())
        .map(|fetch| fetch.filename.as_str())
        .collect();
    let (text, extras) = fetch_empty_named_checksums(&original, client, &already);
    if let Some(easyconfig) = bundle.easyconfigs.first_mut() {
        easyconfig.text = text;
    }
    for fetch in extras {
        if fetch.sha256.is_none() {
            let index = fetched.len();
            bundle.plan.residuals.push(Residual {
                id: format!("source:fetch:{index}"),
                stage: ResidualStage::Normalize,
                category: "checksum".into(),
                severity: ResidualSeverity::Blocking,
                summary: format!(
                    "source {} could not be downloaded and hashed",
                    fetch.filename
                ),
                evidence: Some(fetch.errors.join("; ")),
                provenance: None,
            });
        }
        fetched.push(fetch);
    }
}

fn fetch_empty_named_checksums(
    original: &str,
    client: &dyn RegistryClient,
    already: &std::collections::HashSet<&str>,
) -> (String, Vec<SourceFetch>) {
    let Ok(recipe) = resolve_easyconfig_str(original) else {
        return (original.to_string(), Vec::new());
    };
    let mut text = original.to_string();
    let mut extras = Vec::new();
    for (filename, hash) in &recipe.checksums_by_filename {
        // Dict keys are stored as written. `%(version)s` is still the version
        // the bump just assigned, so expand it before deciding the file is
        // not concrete.
        let concrete = expand_checksum_key(filename, &recipe);
        if !hash.is_empty()
            || concrete.contains("%(")
            || already.contains(concrete.as_str())
            || already.contains(filename.as_str())
        {
            continue;
        }
        let mut fetch = SourceFetch {
            filename: concrete.clone(),
            url: None,
            sha256: None,
            errors: Vec::new(),
            bytes: None,
            respelled: None,
        };
        let file = SourceFile {
            filename: concrete.clone(),
            download_filename: concrete,
            ..SourceFile::default()
        };
        let urls = source_download_urls(&recipe, &file);
        if urls.is_empty() {
            fetch
                .errors
                .push("no source_urls entry to download from".into());
        }
        for url in urls {
            match client.get(&url) {
                Ok(bytes) if !bytes.is_empty() => {
                    let digest = sha256_hex(&bytes);
                    let updated = replace_empty_named_checksum(&text, &recipe, filename, &digest);
                    if updated == text {
                        fetch
                            .errors
                            .push("checksum key is not an empty entry in the recipe".into());
                    } else {
                        text = updated;
                        fetch.sha256 = Some(digest);
                        fetch.url = Some(url);
                    }
                    break;
                }
                Ok(_) => fetch.errors.push(format!("{url}: empty body")),
                Err(error) => fetch.errors.push(error.to_string()),
            }
        }
        extras.push(fetch);
    }
    (text, extras)
}

fn replace_empty_named_checksum(
    text: &str,
    recipe: &ResolvedEasyconfig,
    filename: &str,
    digest: &str,
) -> String {
    // The dict key in the file may still be a template (`go%(version)s...`)
    // whose resolved form is `filename`. Try the resolved name, then any
    // empty key that expands to it.
    let direct = replace_empty_key(text, filename, digest);
    if direct != text {
        return direct;
    }
    let Some(keys) = empty_checksum_keys(text) else {
        return text.to_string();
    };
    for key in keys {
        if expand_checksum_key(&key, recipe) == filename {
            return replace_empty_key(text, &key, digest);
        }
    }
    text.to_string()
}

/// `%(version)s` / `%(name)s` / `%(namelower)s` as the parser fills them.
/// Enough to recognize a checksum-dict key; anything still containing `%(`
/// is not a concrete filename.
fn expand_checksum_key(key: &str, recipe: &ResolvedEasyconfig) -> String {
    let namelower = recipe.name.to_ascii_lowercase();
    let letter = recipe
        .name
        .chars()
        .next()
        .map(|ch| ch.to_string())
        .unwrap_or_default();
    key.replace("%(version)s", &recipe.version)
        .replace("%(namelower)s", &namelower)
        .replace("%(nameletter)s", &letter)
        .replace("%(name)s", &recipe.name)
        .replace("%(arch)s", std::env::consts::ARCH)
}

fn empty_checksum_keys(text: &str) -> Option<Vec<String>> {
    let re = regex::Regex::new(r#"['"](?P<key>[^'"]+)['"]\s*:\s*['"]['"]"#).ok()?;
    Some(
        re.captures_iter(text)
            .filter_map(|caps| caps.name("key").map(|m| m.as_str().to_string()))
            .collect(),
    )
}

fn replace_empty_key(text: &str, key: &str, digest: &str) -> String {
    // The regex crate has no backreferences, so each quote style is its own
    // pattern. A key and its empty value may use different quotes.
    let escaped = regex::escape(key);
    let mut text = text.to_string();
    for key_quote in ['\'', '"'] {
        for value_quote in ['\'', '"'] {
            let pattern = format!(
                r#"(?P<head>{key}{escaped}{key}\s*:\s*){value}{value}"#,
                key = regex::escape(&key_quote.to_string()),
                value = regex::escape(&value_quote.to_string()),
            );
            let Ok(re) = regex::Regex::new(&pattern) else {
                continue;
            };
            text = re
                .replace_all(&text, |caps: &regex::Captures| {
                    let head = caps.name("head").map(|m| m.as_str()).unwrap_or("");
                    format!("{head}{value_quote}{digest}{value_quote}")
                })
                .into_owned();
        }
    }
    text
}

/// What the emitted recipe already says about one later source's digest.
enum StatedChecksum {
    /// The recipe states this digest: the download confirms it.
    Matches,
    /// The recipe keys an empty entry by this file: the digest can go in.
    EmptyNamed,
    /// The recipe states another digest, or none that can be found.
    Other,
}

fn stated_checksum(
    recipe: &ResolvedEasyconfig,
    index: usize,
    filename: &str,
    digest: &str,
) -> StatedChecksum {
    let named = recipe
        .checksums_by_filename
        .iter()
        .find(|(key, _)| key.as_str() == filename || expand_checksum_key(key, recipe) == filename)
        .map(|(_, value)| value.as_str());
    match named {
        Some(value) if value.eq_ignore_ascii_case(digest) => return StatedChecksum::Matches,
        Some("") => return StatedChecksum::EmptyNamed,
        Some(_) => return StatedChecksum::Other,
        None => {}
    }
    // A purely positional list lines up with `sources`, entry for entry.
    let positional = !recipe.checksum_entry_keys.is_empty()
        && recipe.checksum_entry_keys.iter().all(Vec::is_empty);
    if positional
        && recipe
            .checksums
            .get(index)
            .is_some_and(|value| value.eq_ignore_ascii_case(digest))
    {
        return StatedChecksum::Matches;
    }
    StatedChecksum::Other
}

/// Residuals for every source after the first `applied` ones.
///
/// A later source keeps the digest the recipe states for it. When the
/// download hashes to that digest the entry is confirmed, not a residual;
/// when the recipe keys an empty entry by the file, the digest is written
/// there. Only a digest the recipe contradicts, or cannot be matched to an
/// entry, stays a residual.
fn push_fetch_residuals(bundle: &mut PackageBundle, fetched: &[SourceFetch], applied: usize) {
    let recipe = bundle
        .easyconfigs
        .first()
        .and_then(|easyconfig| resolve_easyconfig_str(&easyconfig.text).ok());
    for (index, fetch) in fetched.iter().enumerate().skip(applied) {
        let (summary, evidence) = match &fetch.sha256 {
            Some(digest) => {
                let stated = recipe
                    .as_ref()
                    .map(|recipe| stated_checksum(recipe, index, &fetch.filename, digest))
                    .unwrap_or(StatedChecksum::Other);
                match stated {
                    StatedChecksum::Matches => continue,
                    StatedChecksum::EmptyNamed => {
                        if let (Some(recipe), Some(easyconfig)) =
                            (recipe.as_ref(), bundle.easyconfigs.first_mut())
                        {
                            let updated = replace_empty_named_checksum(
                                &easyconfig.text,
                                recipe,
                                &fetch.filename,
                                digest,
                            );
                            if updated != easyconfig.text {
                                easyconfig.text = updated;
                                continue;
                            }
                        }
                    }
                    StatedChecksum::Other => {}
                }
                (
                    format!(
                        "source {} was downloaded but the recipe states no matching checksum for it",
                        fetch.filename
                    ),
                    format!(
                        "sha256 {digest} from {}",
                        fetch.url.as_deref().unwrap_or("")
                    ),
                )
            }
            None => (
                format!(
                    "source {} could not be downloaded and hashed",
                    fetch.filename
                ),
                fetch.errors.join("; "),
            ),
        };
        bundle.plan.residuals.push(Residual {
            id: format!("source:fetch:{index}"),
            stage: ResidualStage::Normalize,
            category: "checksum".into(),
            severity: ResidualSeverity::Blocking,
            summary,
            evidence: Some(evidence),
            provenance: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MapClient;

    const RECIPE: &str = r#"
easyblock = 'ConfigureMake'
name = 'Example'
version = '2.0'
homepage = 'https://example.org'
description = 'x'
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
source_urls = ['https://mirror-a.example.org/src', 'https://mirror-b.example.org/src/']
sources = [SOURCELOWER_TAR_GZ, {'filename': 'extra-%(version)s.tar.gz', 'download_filename': 'v%(version)s.tar.gz', 'source_urls': ['https://github.com/org/extra/archive']}]
checksums = ['', '']
moduleclass = 'tools'
"#;

    #[test]
    fn sources_resolve_with_dict_download_names_and_own_urls() {
        let recipe = resolve_easyconfig_str(RECIPE).unwrap();
        assert_eq!(recipe.source_files.len(), 2);
        assert_eq!(recipe.source_files[0].filename, "example-2.0.tar.gz");
        assert_eq!(recipe.source_files[1].filename, "extra-2.0.tar.gz");
        assert_eq!(recipe.source_files[1].download_filename, "v2.0.tar.gz");
        assert_eq!(
            source_download_urls(&recipe, &recipe.source_files[0]),
            vec![
                "https://mirror-a.example.org/src/example-2.0.tar.gz".to_string(),
                "https://mirror-b.example.org/src/example-2.0.tar.gz".to_string(),
            ]
        );
        assert_eq!(
            source_download_urls(&recipe, &recipe.source_files[1]),
            vec!["https://github.com/org/extra/archive/v2.0.tar.gz".to_string()]
        );
    }

    #[test]
    fn a_failing_mirror_falls_through_to_the_next_and_the_digest_is_the_bytes() {
        let recipe = resolve_easyconfig_str(RECIPE).unwrap();
        let mut client = MapClient::default();
        client.pages.insert(
            "https://mirror-b.example.org/src/example-2.0.tar.gz".into(),
            b"abc".to_vec(),
        );
        let fetched = fetch_sources(&recipe, &client);
        assert_eq!(
            fetched[0].sha256.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(
            fetched[0].url.as_deref(),
            Some("https://mirror-b.example.org/src/example-2.0.tar.gz")
        );
        assert_eq!(fetched[0].errors.len(), 1);
        assert!(fetched[1].sha256.is_none());
        assert!(!fetched[1].errors.is_empty());
    }

    fn python_recipe(easyblock: &str, source_urls: &str) -> String {
        format!(
            "easyblock = '{easyblock}'\n\
             name = 'Widget'\n\
             version = '1.2.3'\n\
             homepage = 'https://example.invalid'\n\
             description = 'x'\n\
             toolchain = {{'name': 'GCCcore', 'version': '14.3.0'}}\n\
             {source_urls}\
             sources = [SOURCE_TAR_GZ]\n\
             checksums = ['']\n\
             moduleclass = 'tools'\n"
        )
    }

    #[test]
    fn a_pythonpackage_with_no_source_urls_downloads_from_pypi_source() {
        let recipe = resolve_easyconfig_str(&python_recipe("PythonPackage", "")).unwrap();
        assert!(recipe.source_urls.is_empty());
        let url = "https://pypi.python.org/packages/source/W/Widget/Widget-1.2.3.tar.gz";
        assert_eq!(
            source_download_urls(&recipe, &recipe.source_files[0]),
            vec![url.to_string()]
        );
        let mut client = MapClient::default();
        client.pages.insert(url.into(), b"xyz".to_vec());
        let fetched = fetch_sources(&recipe, &client);
        assert_eq!(
            fetched[0].sha256.as_deref(),
            Some("3608bca1e44ea6c4d268eb6db02260269892c0b42b86bbf1e77a6fa16c3c9282")
        );
        assert_eq!(fetched[0].url.as_deref(), Some(url));
    }

    #[test]
    fn cargo_pythonpackage_and_pythonbundle_inherit_the_pypi_default() {
        for easyblock in ["CargoPythonPackage", "PythonBundle"] {
            let recipe = resolve_easyconfig_str(&python_recipe(easyblock, "")).unwrap();
            assert_eq!(
                source_download_urls(&recipe, &recipe.source_files[0]),
                vec!["https://pypi.python.org/packages/source/W/Widget/Widget-1.2.3.tar.gz"]
            );
        }
    }

    #[test]
    fn an_explicit_source_url_wins_over_the_pythonpackage_default() {
        let recipe = resolve_easyconfig_str(&python_recipe(
            "PythonPackage",
            "source_urls = ['https://mirror.example.org/src']\n",
        ))
        .unwrap();
        assert_eq!(
            source_download_urls(&recipe, &recipe.source_files[0]),
            vec!["https://mirror.example.org/src/Widget-1.2.3.tar.gz"]
        );
    }

    #[test]
    fn an_easyblock_without_a_pypi_default_still_has_no_url() {
        let recipe = resolve_easyconfig_str(&python_recipe("ConfigureMake", "")).unwrap();
        assert!(source_download_urls(&recipe, &recipe.source_files[0]).is_empty());
        let fetched = fetch_sources(&recipe, &MapClient::default());
        assert_eq!(
            fetched[0].errors,
            vec!["no source_urls entry to download from".to_string()]
        );
    }

    #[test]
    fn empty_checksum_dict_keys_are_fetched_from_the_recipe_urls() {
        let text = "\
name = 'Pkg'
version = '1.2.3'
toolchain = SYSTEM
source_urls = ['https://mirror.example.org/src']
sources = ['pkg-%(version)s-linux-%(arch)s.tar.gz']
checksums = [{
    'pkg-1.2.3-linux-amd64.tar.gz': '',
    'pkg-1.2.3-linux-arm64.tar.gz': 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
}]
";
        let mut client = MapClient::default();
        client.pages.insert(
            "https://mirror.example.org/src/pkg-1.2.3-linux-amd64.tar.gz".into(),
            b"aaa".to_vec(),
        );
        let (patched, extras) =
            fetch_empty_named_checksums(text, &client, &std::collections::HashSet::new());
        assert_eq!(
            extras[0].sha256.as_deref(),
            Some("9834876dcfb05cb167a5c24953eba58c4ac89b1adf57f28f2f9d09af107ee8f0")
        );
        assert!(patched.contains(
            "'pkg-1.2.3-linux-amd64.tar.gz': '9834876dcfb05cb167a5c24953eba58c4ac89b1adf57f28f2f9d09af107ee8f0'"
        ));
        // A digest that is already present is not fetched again.
        assert!(patched.contains(
            "'pkg-1.2.3-linux-arm64.tar.gz': 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'"
        ));
    }

    #[test]
    fn an_empty_version_template_key_is_filled_from_the_resolved_filename() {
        let text = "\
name = 'Pkg'
version = '1.2.3'
toolchain = SYSTEM
source_urls = ['https://mirror.example.org/src']
sources = ['pkg-%(version)s-linux-%(arch)s.tar.gz']
checksums = [{
    'pkg-%(version)s-linux-amd64.tar.gz': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    'pkg-%(version)s-linux-arm64.tar.gz': '',
}]
";
        let mut client = MapClient::default();
        client.pages.insert(
            "https://mirror.example.org/src/pkg-1.2.3-linux-arm64.tar.gz".into(),
            b"bbb".to_vec(),
        );
        let (patched, extras) =
            fetch_empty_named_checksums(text, &client, &std::collections::HashSet::new());
        assert_eq!(
            extras[0].sha256.as_deref(),
            Some("3e744b9dc39389baf0c5a0660589b8402f3dbb49b89b3e75f2c9355852a3c677")
        );
        assert!(patched.contains(
            "'pkg-%(version)s-linux-arm64.tar.gz': '3e744b9dc39389baf0c5a0660589b8402f3dbb49b89b3e75f2c9355852a3c677'"
        ));
        assert!(patched.contains("'pkg-%(version)s-linux-amd64.tar.gz': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'"));
    }

    #[test]
    fn a_git_checkout_is_never_hashed() {
        let text = RECIPE.replace(
            "'source_urls': ['https://github.com/org/extra/archive']",
            "'git_config': {'url': 'https://github.com/org', 'repo_name': 'extra', 'tag': 'v2.0'}",
        );
        let recipe = resolve_easyconfig_str(&text).unwrap();
        let fetched = fetch_sources(&recipe, &MapClient::default());
        assert!(fetched[1].sha256.is_none());
        assert!(fetched[1].errors[0].starts_with("git_config"));
    }

    const SECOND_SOURCE: &str = "\
name = 'gc'
version = '8.2.12'
local_libatomic_version = '7.8.2'
toolchain = SYSTEM
source_urls = ['https://example.org/']
sources = [SOURCE_TAR_GZ, 'libatomic_ops-%s.tar.gz' % local_libatomic_version]
checksums = [
    {'gc-8.2.12.tar.gz': 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'},
    {'libatomic_ops-7.8.2.tar.gz': 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'},
]
";

    #[test]
    fn a_second_source_whose_download_matches_the_stated_digest_is_confirmed() {
        let recipe = resolve_easyconfig_str(SECOND_SOURCE).unwrap();
        let digest = "b".repeat(64);
        assert!(matches!(
            stated_checksum(&recipe, 1, "libatomic_ops-7.8.2.tar.gz", &digest),
            StatedChecksum::Matches
        ));
        assert!(matches!(
            stated_checksum(&recipe, 1, "libatomic_ops-7.8.2.tar.gz", &"c".repeat(64)),
            StatedChecksum::Other
        ));
    }

    #[test]
    fn a_second_source_with_an_empty_named_entry_can_take_the_digest() {
        let text = SECOND_SOURCE.replace(&"b".repeat(64), "");
        let recipe = resolve_easyconfig_str(&text).unwrap();
        assert!(matches!(
            stated_checksum(&recipe, 1, "libatomic_ops-7.8.2.tar.gz", &"c".repeat(64)),
            StatedChecksum::EmptyNamed
        ));
    }

    #[test]
    fn an_archive_tag_spelled_with_a_v_is_fetched_and_the_source_respelled() {
        let text = "\
easyblock = 'CMakeMake'
name = 'GOTCHA'
version = '1.0.10'
homepage = 'https://example.org'
description = 'x'
toolchain = SYSTEM
source_urls = ['https://github.com/LLNL/GOTCHA/archive/']
sources = ['%(version)s.tar.gz']
checksums = ['']
moduleclass = 'devel'
";
        let recipe = resolve_easyconfig_str(text).unwrap();
        let mut client = MapClient::default();
        client.pages.insert(
            "https://github.com/LLNL/GOTCHA/archive/v1.0.10.tar.gz".into(),
            b"gotcha".to_vec(),
        );
        let fetched = fetch_sources(&recipe, &client);
        assert_eq!(
            fetched[0].sha256.as_deref(),
            Some(sha256_hex(b"gotcha").as_str())
        );
        assert_eq!(fetched[0].filename, "v1.0.10.tar.gz");
        let respelling = fetched[0].respelled.as_ref().expect("respelled");
        assert_eq!(respelling.from, "1.0.10.tar.gz");
        assert_eq!(respelling.to, "v1.0.10.tar.gz");
        let body = "['%(version)s.tar.gz']";
        assert_eq!(
            respell_literals(body, "1.0.10", "1.0.10.tar.gz", true),
            "['v%(version)s.tar.gz']"
        );
    }

    #[test]
    fn a_download_name_not_led_by_the_version_is_not_respelled() {
        let bases = vec!["https://github.com/o/r/archive/".to_string()];
        assert_eq!(other_tag_spelling(&bases, "r-1.0.tar.gz", "1.0"), None);
        assert_eq!(
            other_tag_spelling(&bases, "v1.0.tar.gz", "1.0").as_deref(),
            Some("1.0.tar.gz")
        );
        let release = vec!["https://example.org/releases/".to_string()];
        assert_eq!(other_tag_spelling(&release, "1.0.tar.gz", "1.0"), None);
    }

    #[test]
    fn an_empty_checksum_anywhere_keeps_the_cleared_warning() {
        assert!(has_empty_checksum("checksums = ['']\n"));
        assert!(has_empty_checksum(
            "exts_list = [(name, version, {'checksums': [{'x-1.tar.gz': ''}]})]\n"
        ));
        assert!(!has_empty_checksum(
            "checksums = [{'x-1.tar.gz': 'aaaa'}]\nexts_list = [(name, version, {'checksums': ['bbbb']})]\n"
        ));
    }

    #[test]
    fn a_positional_second_checksum_is_matched_by_position() {
        let text = "\
name = 'gc'
version = '8.2.12'
toolchain = SYSTEM
source_urls = ['https://example.org/']
sources = [SOURCE_TAR_GZ, 'libatomic_ops-7.8.2.tar.gz']
checksums = [
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
]
";
        let recipe = resolve_easyconfig_str(text).unwrap();
        assert!(matches!(
            stated_checksum(&recipe, 1, "libatomic_ops-7.8.2.tar.gz", &"b".repeat(64)),
            StatedChecksum::Matches
        ));
    }
}
