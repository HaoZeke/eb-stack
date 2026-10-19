//! Fill the source checksum a version bump invalidates, the way
//! `eb --inject-checksums` does: download each `sources` entry from the
//! recipe's own `source_urls`, in order, and hash the bytes.
//!
//! A bump that changes the version clears the old digest because it is not a
//! claim about the new tarball. EasyBuild's answer is `--inject-checksums`,
//! which fetches the artifact the recipe names and writes its SHA-256. This
//! module does the same before emission, so the bumped recipe carries a real
//! digest and the SBOM's source component carries the same hash. Nothing here
//! guesses: an entry with no URL, a git checkout, or a failed download stays
//! unresolved and is reported as a residual with the reason, never filled.

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
/// `source_urls` when the dict sets them, else the recipe's.
pub fn source_download_urls(recipe: &ResolvedEasyconfig, file: &SourceFile) -> Vec<String> {
    let bases = if file.source_urls.is_empty() {
        &recipe.source_urls
    } else {
        &file.source_urls
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
    let fetched = fetch_sources(&recipe, client);
    let Some(digest) = fetched.first().and_then(|fetch| fetch.sha256.clone()) else {
        let mut bundle = first;
        push_fetch_residuals(&mut bundle, &fetched, 0);
        return Ok((bundle, fetched));
    };
    let mut again = request.clone();
    again.source_checksum = Some(digest);
    let mut bundle = plan_package_bump_with(&again, options)?;
    push_fetch_residuals(&mut bundle, &fetched, 1);
    Ok((bundle, fetched))
}

fn push_fetch_residuals(bundle: &mut PackageBundle, fetched: &[SourceFetch], applied: usize) {
    for (index, fetch) in fetched.iter().enumerate().skip(applied) {
        let (summary, evidence) = match &fetch.sha256 {
            Some(digest) => (
                format!(
                    "source {} was downloaded but only the first source checksum is rewritten",
                    fetch.filename
                ),
                format!("sha256 {digest} from {}", fetch.url.as_deref().unwrap_or("")),
            ),
            None => (
                format!("source {} could not be downloaded and hashed", fetch.filename),
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
