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
                        break;
                    }
                    Ok(_) => fetch.errors.push(format!("{url}: empty body")),
                    Err(error) => fetch.errors.push(error.to_string()),
                }
            }
            fetch
        })
        .collect()
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
pub fn plan_package_bump_fetching(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: &dyn RegistryClient,
) -> Result<(PackageBundle, Vec<SourceFetch>), PackageWorkflowError> {
    let first = plan_package_bump_with(request, options)?;
    if request.version.is_none() || request.source_checksum.is_some() {
        return Ok((first, Vec::new()));
    }
    let Some(emitted) = first.easyconfigs.first() else {
        return Ok((first, Vec::new()));
    };
    let recipe = resolve_easyconfig_str(&emitted.text)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    let mut fetched = fetch_sources(&recipe, client);
    let Some(digest) = fetched.first().and_then(|fetch| fetch.sha256.clone()) else {
        let mut bundle = first;
        push_fetch_residuals(&mut bundle, &fetched, 0);
        fill_empty_named_checksums(&mut bundle, client, &mut fetched);
        return Ok((bundle, fetched));
    };
    let mut again = request.clone();
    again.source_checksum = Some(digest);
    let mut bundle = plan_package_bump_with(&again, options)?;
    push_fetch_residuals(&mut bundle, &fetched, 1);
    fill_empty_named_checksums(&mut bundle, client, &mut fetched);
    Ok((bundle, fetched))
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

fn push_fetch_residuals(bundle: &mut PackageBundle, fetched: &[SourceFetch], applied: usize) {
    for (index, fetch) in fetched.iter().enumerate().skip(applied) {
        let (summary, evidence) = match &fetch.sha256 {
            Some(digest) => (
                format!(
                    "source {} was downloaded but only the first source checksum is rewritten",
                    fetch.filename
                ),
                format!(
                    "sha256 {digest} from {}",
                    fetch.url.as_deref().unwrap_or("")
                ),
            ),
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
}
