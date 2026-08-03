//! What kind of artifact a source URL denotes, and whether a checksum seeded
//! from another ecosystem was computed over that same kind.
//!
//! A version number does not identify bytes. GitHub serves two different
//! tarballs for one tag: the archive it generates from the tree
//! (`/archive/refs/tags/v1.2.3.tar.gz`) and whatever the project uploaded as a
//! release asset (`/releases/download/v1.2.3/thing-1.2.3.src.tar.xz`). Spack
//! and conda-forge frequently hash the first; EasyBuild frequently fetches the
//! second. Copying a checksum across therefore yields a hash that is valid,
//! well-formed, and wrong, and the build only finds out after the download.
//!
//! Classifying both ends and refusing to carry a checksum between classes is
//! what turns that into a build-time error instead of a silent one.

use std::fmt;

/// The kind of artifact a source URL resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactClass {
    /// A tarball GitHub generates from a tag or branch: `/archive/...`.
    GitHubTagArchive,
    /// A file the project uploaded to a release: `/releases/download/...`.
    GitHubReleaseAsset,
    /// A source distribution from the Python package index.
    PyPiSdist,
    /// A SourceForge file release.
    SourceForge,
    /// A checkout rather than a downloaded file. Has no artifact checksum.
    GitCheckout,
    /// A recognised URL whose class carries no cross-ecosystem hazard.
    Other,
    /// Nothing to classify: no URL, or one that parses as nothing useful.
    Unknown,
}

impl ArtifactClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactClass::GitHubTagArchive => "github-tag-archive",
            ArtifactClass::GitHubReleaseAsset => "github-release-asset",
            ArtifactClass::PyPiSdist => "pypi-sdist",
            ArtifactClass::SourceForge => "sourceforge",
            ArtifactClass::GitCheckout => "git-checkout",
            ArtifactClass::Other => "other",
            ArtifactClass::Unknown => "unknown",
        }
    }

    /// Whether a checksum may be carried from `self` to `other`.
    ///
    /// Only equal classes are compatible. `Unknown` never certifies anything:
    /// an unclassified end means the question was not answered, which is not
    /// the same as answered yes.
    pub fn checksum_transfers_to(self, other: ArtifactClass) -> bool {
        self != ArtifactClass::Unknown && self == other
    }

    /// Whether two different classes are known to serve different bytes for
    /// the same version. This is the mismatch worth failing a check over.
    pub fn conflicts_with(self, other: ArtifactClass) -> bool {
        use ArtifactClass::*;
        if self == Unknown || other == Unknown || self == other {
            return false;
        }
        // A checkout has no artifact hash at all, so pairing it with a
        // downloaded file is a category error rather than a byte mismatch.
        matches!(
            (self, other),
            (GitHubTagArchive, GitHubReleaseAsset)
                | (GitHubReleaseAsset, GitHubTagArchive)
                | (GitHubTagArchive, PyPiSdist)
                | (PyPiSdist, GitHubTagArchive)
                | (GitHubReleaseAsset, PyPiSdist)
                | (PyPiSdist, GitHubReleaseAsset)
                | (GitHubTagArchive, SourceForge)
                | (SourceForge, GitHubTagArchive)
                | (GitHubReleaseAsset, SourceForge)
                | (SourceForge, GitHubReleaseAsset)
                | (PyPiSdist, SourceForge)
                | (SourceForge, PyPiSdist)
        )
    }
}

impl fmt::Display for ArtifactClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify a download URL.
///
/// `url` may be a full URL or an EasyBuild `source_urls` entry with the
/// filename appended separately; both are matched on path shape rather than on
/// the filename, because the filename alone does not distinguish an archive
/// from an asset.
pub fn classify_url(url: &str) -> ArtifactClass {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return ArtifactClass::Unknown;
    }
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("git://")
        || lower.starts_with("git+")
        || lower.ends_with(".git")
        || lower.starts_with("ssh://git@")
    {
        return ArtifactClass::GitCheckout;
    }

    let host_and_path = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&lower);
    let (host, path) = host_and_path.split_once('/').unwrap_or((host_and_path, ""));

    if host.contains("github.com") || host.contains("codeload.github.com") {
        if path.contains("/releases/download/") {
            return ArtifactClass::GitHubReleaseAsset;
        }
        // `/archive`, `/archive/refs/tags/...`, `/tar.gz/...` are all the
        // generated-tree tarball.
        if path.contains("/archive") || path.contains("/tar.gz/") || path.contains("/zip/") {
            return ArtifactClass::GitHubTagArchive;
        }
        return ArtifactClass::Other;
    }

    if host.contains("pythonhosted.org")
        || host.contains("pypi.python.org")
        || host.contains("pypi.io")
        || host.contains("pypi.org")
    {
        return ArtifactClass::PyPiSdist;
    }

    if host.contains("sourceforge.net") || host.ends_with(".sf.net") {
        return ArtifactClass::SourceForge;
    }

    ArtifactClass::Other
}

/// Classify a foreign recipe's source, which may name a checkout instead of a
/// download.
pub fn classify_foreign(url: Option<&str>, git: Option<&str>) -> ArtifactClass {
    if let Some(git) = git {
        if !git.trim().is_empty() {
            return ArtifactClass::GitCheckout;
        }
    }
    match url {
        Some(u) => classify_url(u),
        None => ArtifactClass::Unknown,
    }
}

/// How serious a source-verification finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingLevel {
    /// The checksum is known to describe different bytes than the source.
    Error,
    /// The question could not be answered, so the checksum is unverified.
    Warning,
}

/// One statement about a source and the checksum attached to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFinding {
    pub level: FindingLevel,
    pub message: String,
}

impl SourceFinding {
    fn error(message: String) -> Self {
        Self {
            level: FindingLevel::Error,
            message,
        }
    }

    fn warning(message: String) -> Self {
        Self {
            level: FindingLevel::Warning,
            message,
        }
    }
}

impl fmt::Display for SourceFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.level {
            FindingLevel::Error => "err",
            FindingLevel::Warning => "warn",
        };
        write!(f, "[{tag}] {}", self.message)
    }
}

/// Where a checksum came from, when that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededChecksum {
    /// Ecosystem the value was copied from, for the message.
    pub origin: String,
    /// The URL that value was computed over.
    pub source_url: Option<String>,
    /// A git remote, when the foreign recipe built from a checkout.
    pub git: Option<String>,
    pub sha256: Option<String>,
}

/// Verify a recipe's source URLs against a checksum seeded from elsewhere.
///
/// With no seed, the sources are still classified and anything unclassifiable
/// is reported, because "we could not tell" is the state in which a wrong
/// checksum survives.
pub fn verify_sources(
    source_urls: &[String],
    seeded: Option<&SeededChecksum>,
) -> Vec<SourceFinding> {
    let mut findings = Vec::new();

    if source_urls.is_empty() {
        findings.push(SourceFinding::warning(
            "no source_urls to classify: a seeded checksum cannot be verified against \
             the artifact this recipe downloads"
                .into(),
        ));
    }

    let classes: Vec<(String, ArtifactClass)> = source_urls
        .iter()
        .map(|u| (u.clone(), classify_url(u)))
        .collect();

    for (url, class) in &classes {
        if *class == ArtifactClass::Unknown {
            findings.push(SourceFinding::warning(format!(
                "source URL {url:?} could not be classified"
            )));
        }
    }

    let Some(seed) = seeded else {
        return findings;
    };

    let seed_class = classify_foreign(seed.source_url.as_deref(), seed.git.as_deref());
    if seed_class == ArtifactClass::Unknown {
        findings.push(SourceFinding::warning(format!(
            "checksum seeded from {} has no classifiable source, so it is unverified",
            seed.origin
        )));
        return findings;
    }

    if seed_class == ArtifactClass::GitCheckout {
        findings.push(SourceFinding::error(format!(
            "checksum seeded from {} was taken from a git checkout ({}), which has no \
             artifact hash; it cannot describe a downloaded file",
            seed.origin,
            seed.git.as_deref().unwrap_or("unknown remote")
        )));
        return findings;
    }

    // A recipe may list mirrors of one artifact, so the seed is compatible if
    // it matches any listed source, and conflicting only if it conflicts with
    // every one of them.
    if classes
        .iter()
        .any(|(_, class)| seed_class.checksum_transfers_to(*class))
    {
        return findings;
    }

    for (url, class) in &classes {
        if seed_class.conflicts_with(*class) {
            findings.push(SourceFinding::error(format!(
                "checksum seeded from {} is a {seed_class} hash ({}), but this recipe \
                 downloads a {class} from {url:?}; the same version serves different \
                 bytes for these two, so the value is wrong",
                seed.origin,
                seed.sha256.as_deref().unwrap_or("no value"),
            )));
        } else if *class != ArtifactClass::Unknown {
            findings.push(SourceFinding::warning(format!(
                "checksum seeded from {} is a {seed_class} hash but this recipe \
                 downloads a {class} from {url:?}; the classes differ and the value is \
                 unverified",
                seed.origin
            )));
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_archive_and_release_are_different_classes() {
        // The exact pair that makes a copied checksum wrong.
        assert_eq!(
            classify_url(
                "https://github.com/llvm/llvm-project/archive/refs/tags/llvmorg-22.1.8.tar.gz"
            ),
            ArtifactClass::GitHubTagArchive
        );
        assert_eq!(
            classify_url(
                "https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/llvm-project-22.1.8.src.tar.xz"
            ),
            ArtifactClass::GitHubReleaseAsset
        );
        assert!(ArtifactClass::GitHubTagArchive.conflicts_with(ArtifactClass::GitHubReleaseAsset));
        assert!(!ArtifactClass::GitHubTagArchive
            .checksum_transfers_to(ArtifactClass::GitHubReleaseAsset));
    }

    #[test]
    fn the_easybuild_url_constants_classify_as_their_names_promise() {
        // GITHUB_SOURCE / GITHUB_LOWER_SOURCE expand to the archive endpoint.
        assert_eq!(
            classify_url("https://github.com/acct/Name/archive"),
            ArtifactClass::GitHubTagArchive
        );
        // GITHUB_RELEASE / GITHUB_LOWER_RELEASE expand to the asset endpoint.
        assert_eq!(
            classify_url("https://github.com/acct/Name/releases/download/v1.2.3"),
            ArtifactClass::GitHubReleaseAsset
        );
        assert_eq!(
            classify_url("https://pypi.python.org/packages/source/N/Name"),
            ArtifactClass::PyPiSdist
        );
        assert_eq!(
            classify_url("https://download.sourceforge.net/thing"),
            ArtifactClass::SourceForge
        );
    }

    #[test]
    fn codeload_and_tarball_shapes_are_still_archives() {
        for url in [
            "https://codeload.github.com/acct/Name/tar.gz/refs/tags/v1.0",
            "https://github.com/acct/Name/archive/v1.0.tar.gz",
            "https://github.com/acct/Name/zip/refs/heads/main",
        ] {
            assert_eq!(classify_url(url), ArtifactClass::GitHubTagArchive, "{url}");
        }
    }

    #[test]
    fn checkouts_are_recognised_however_they_are_written() {
        for url in [
            "git://github.com/acct/Name.git",
            "https://github.com/acct/Name.git",
            "git+https://github.com/acct/Name",
            "ssh://git@github.com/acct/Name",
        ] {
            assert_eq!(classify_url(url), ArtifactClass::GitCheckout, "{url}");
        }
        assert_eq!(
            classify_foreign(None, Some("https://github.com/acct/Name")),
            ArtifactClass::GitCheckout
        );
        assert_eq!(classify_foreign(None, None), ArtifactClass::Unknown);
        assert_eq!(classify_foreign(None, Some("  ")), ArtifactClass::Unknown);
    }

    #[test]
    fn an_empty_or_opaque_url_is_unknown_not_compatible() {
        assert_eq!(classify_url(""), ArtifactClass::Unknown);
        assert_eq!(classify_url("   "), ArtifactClass::Unknown);
        // Unknown must never certify a transfer: not answering is not a yes.
        assert!(!ArtifactClass::Unknown.checksum_transfers_to(ArtifactClass::Unknown));
        assert!(!ArtifactClass::Unknown.conflicts_with(ArtifactClass::GitHubTagArchive));
    }

    fn seed(url: &str) -> SeededChecksum {
        SeededChecksum {
            origin: "conda-forge".into(),
            source_url: Some(url.into()),
            git: None,
            sha256: Some("a".repeat(64)),
        }
    }

    #[test]
    fn a_tag_archive_checksum_on_a_release_asset_recipe_is_an_error() {
        // The LLVM 22.1.8 shape: foreign recipe hashed the generated archive,
        // the easyconfig downloads the uploaded asset.
        let findings = verify_sources(
            &["https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8".into()],
            Some(&seed(
                "https://github.com/llvm/llvm-project/archive/refs/tags/llvmorg-22.1.8.tar.gz",
            )),
        );
        let errors: Vec<&SourceFinding> = findings
            .iter()
            .filter(|f| f.level == FindingLevel::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert!(
            errors[0].message.contains("github-tag-archive")
                && errors[0].message.contains("github-release-asset"),
            "{}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("different"),
            "{}",
            errors[0].message
        );
    }

    #[test]
    fn a_matching_class_passes_without_a_finding() {
        let findings = verify_sources(
            &["https://github.com/acct/Name/archive".into()],
            Some(&seed(
                "https://github.com/acct/Name/archive/refs/tags/v1.0.tar.gz",
            )),
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn one_matching_mirror_is_enough() {
        // Recipes list mirrors of the same artifact; matching any of them is a
        // match, and the others must not raise a false alarm.
        let findings = verify_sources(
            &[
                "https://mirror.example.org/pub/name".into(),
                "https://github.com/acct/Name/archive".into(),
            ],
            Some(&seed(
                "https://github.com/acct/Name/archive/refs/tags/v1.0.tar.gz",
            )),
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_checkout_seed_cannot_describe_a_downloaded_file() {
        let mut s = seed("");
        s.source_url = None;
        s.git = Some("https://github.com/acct/Name".into());
        let findings = verify_sources(&["https://github.com/acct/Name/archive".into()], Some(&s));
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].level, FindingLevel::Error);
        assert!(findings[0].message.contains("git checkout"), "{findings:?}");
    }

    #[test]
    fn an_unclassifiable_pairing_warns_rather_than_passing_silently() {
        // Neither a known conflict nor a match: the value is unverified, and
        // saying nothing is what let a wrong checksum through before.
        let findings = verify_sources(
            &["https://example.org/downloads/name-1.0.tar.gz".into()],
            Some(&seed("https://github.com/acct/Name/archive")),
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].level, FindingLevel::Warning);
        assert!(findings[0].message.contains("unverified"), "{findings:?}");
    }

    #[test]
    fn no_sources_to_check_is_itself_reported() {
        let findings = verify_sources(&[], Some(&seed("https://github.com/acct/Name/archive")));
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("no source_urls to classify")),
            "{findings:?}"
        );
    }

    #[test]
    fn without_a_seed_the_sources_are_still_classified() {
        assert!(verify_sources(&["https://github.com/acct/Name/archive".into()], None).is_empty());
        let findings = verify_sources(&["".into()], None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].level, FindingLevel::Warning);
    }
}
