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
use std::path::Path;

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
    /// The stable lowercase name used in output and messages.
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

    if host_matches(host, "github.com") || host_matches(host, "codeload.github.com") {
        if path.contains("/releases/download/") {
            return ArtifactClass::GitHubReleaseAsset;
        }
        // `/archive`, `/archive/refs/tags/...`, `/tar.gz/...` are all the
        // generated-tree tarball.
        if path_has_segment(path, "archive") || path.contains("/tar.gz/") || path.contains("/zip/")
        {
            return ArtifactClass::GitHubTagArchive;
        }
        return ArtifactClass::Other;
    }

    if host_matches(host, "pythonhosted.org")
        || host_matches(host, "pypi.python.org")
        || host_matches(host, "pypi.io")
        || host_matches(host, "pypi.org")
    {
        if path.ends_with(".whl") || path.contains("-py2-") || path.contains("-py3-") {
            return ArtifactClass::Other;
        }
        return ArtifactClass::PyPiSdist;
    }

    if host_matches(host, "sourceforge.net") || host.ends_with(".sf.net") {
        return ArtifactClass::SourceForge;
    }

    ArtifactClass::Other
}

/// Classify a foreign recipe's source, which may name a checkout instead of a
/// download.
///
/// A GitHub tag-archive or release-asset URL wins over a git remote. Spack
/// (and similar) attach both so the file can be hashed; the remote does not
/// make that hash a checkout hash.
pub fn classify_foreign(url: Option<&str>, git: Option<&str>) -> ArtifactClass {
    if let Some(u) = url {
        let class = classify_url(u);
        // A classifiable download (PyPI, SourceForge, GitHub archive, Other)
        // is the hash subject. Spack attaches git= so the file can be hashed;
        // the remote does not make that hash a checkout hash.
        if class != ArtifactClass::Unknown && class != ArtifactClass::GitCheckout {
            return class;
        }
        if class == ArtifactClass::GitCheckout {
            return ArtifactClass::GitCheckout;
        }
    }
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
    /// Whether this is a known mismatch or merely unverified.
    pub level: FindingLevel,
    /// What was found, in terms a reviewer can act on.
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
    /// The value copied across, quoted back in a mismatch message.
    pub sha256: Option<String>,
}

/// Reads a declared version out of one build system's file, given its text.
type VersionReader = fn(&str) -> Option<String>;

/// The version an upstream build system declares for itself.
///
/// A recipe pinned to a commit takes its version from whoever writes the
/// easyconfig, and the obvious choice, the last release tag, is often wrong:
/// projects bump the in-tree version as soon as a release branches, so a
/// snapshot taken after a tag declares something else. The module then claims a
/// version its own binary does not report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredVersion {
    /// The version string as the build system writes it.
    pub value: String,
    /// The file and construct it came from, for the message.
    pub source: String,
}

/// Read the version an upstream source tree declares, if any build system in it
/// says so plainly.
///
/// Only unambiguous, single-line declarations are read. A version assembled
/// from variables is left alone rather than guessed at, because a wrong answer
/// here would rename a module.
pub fn declared_version(source_tree: &Path) -> Option<DeclaredVersion> {
    let readers: &[(&str, VersionReader)] = &[
        ("CMakeLists.txt", cmake_project_version),
        ("Cargo.toml", toml_package_version),
        ("pyproject.toml", toml_package_version),
        ("meson.build", meson_project_version),
        ("configure.ac", autoconf_init_version),
    ];
    for (name, read) in readers {
        let path = source_tree.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(value) = read(&text) {
            return Some(DeclaredVersion {
                value,
                source: (*name).to_string(),
            });
        }
    }
    None
}

/// `project(name VERSION 4.3.9 LANGUAGES C CXX)`, across lines.
///
/// Commented `project()` is skipped. A `)` inside a quoted DESCRIPTION does
/// not end the command.
fn cmake_project_version(text: &str) -> Option<String> {
    static VER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let ver = VER.get_or_init(|| {
        regex::Regex::new(r"(?i)\bVERSION\s+([0-9][0-9A-Za-z.\-+]*)")
            .expect("literal cmake version")
    });
    let mut from = 0;
    while let Some(body) = next_cmake_project_body(text, &mut from) {
        let unquoted = cmake_without_quoted_strings(body);
        if let Some(caps) = ver.captures(&unquoted) {
            return Some(caps.get(1)?.as_str().to_string());
        }
    }
    None
}

/// Replace double-quoted regions with spaces so `VERSION` inside DESCRIPTION
/// text is not a keyword.
fn cmake_without_quoted_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            out.push(' ');
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

/// Next `project(...)` argument body after `from`, skipping comments and
/// matching parentheses outside quoted strings. Advances `from` past it.
fn next_cmake_project_body<'a>(text: &'a str, from: &mut usize) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = *from;
    let mut in_string = false;
    let mut escaped = false;
    while i < n {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if matches!(c, b'p' | b'P')
            && i + 7 <= n
            && text[i..i + 7].eq_ignore_ascii_case("project")
            && (i == 0 || !cmake_ident_byte(bytes[i - 1]))
        {
            let mut j = i + 7;
            while j < n && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < n && bytes[j] == b'(' {
                if let Some(close) = cmake_matching_paren(text, j) {
                    *from = close + 1;
                    return Some(&text[j + 1..close]);
                }
            }
        }
        i += 1;
    }
    *from = n;
    None
}

fn cmake_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Index of the `)` that closes the `(` at `open`, ignoring parens inside
/// quotes and `#` comments.
fn cmake_matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = open + 1;
    let mut depth = 1usize;
    let mut in_string = false;
    let mut escaped = false;
    while i < n && depth > 0 {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `#` to end of line, unless the `#` is inside quotes.
fn strip_inline_comment(line: &str) -> &str {
    let mut in_quote = None;
    for (index, character) in line.char_indices() {
        match (character, in_quote) {
            ('#', None) => return line[..index].trim_end(),
            ('\'' | '"', None) => in_quote = Some(character),
            (quote, Some(open)) if quote == open => in_quote = None,
            _ => {}
        }
    }
    line
}

fn host_matches(host: &str, name: &str) -> bool {
    host == name || host.ends_with(&format!(".{name}"))
}

fn path_has_segment(path: &str, segment: &str) -> bool {
    path.split('/').any(|part| part == segment)
}

/// `version = "1.2.3"` in `[package]` or `[project]`, not the first hit in the file.
///
/// Inline comments are stripped. `version = { workspace = true }` is inherit,
/// not a version string; cargo.rs resolves that form.
fn toml_package_version(text: &str) -> Option<String> {
    let mut section = "";
    for line in text.lines() {
        let trimmed = strip_inline_comment(line.trim());
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed;
            continue;
        }
        if !(section == "[package]" || section == "[project]" || section == "[workspace.package]") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if rest.starts_with('{') {
                    return None;
                }
                let rest = rest.trim_matches('"').trim_matches('\'');
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// `project('name', 'c', version : '1.2.3')`.
///
/// Only a word-boundary `project (` call is scanned. `subproject(...)` and a
/// later `dependency(..., version: ...)` are not a declared project version.
fn meson_project_version(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(open) = meson_next_project_paren(text, from) {
        let Some(close) = meson_matching_paren(text, open) else {
            from = open + 1;
            continue;
        };
        if let Some(version) = meson_version_in_call(&text[open + 1..close]) {
            return Some(version);
        }
        from = close + 1;
    }
    None
}

fn meson_next_project_paren(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = from;
    let mut in_quote = None;
    let mut escaped = false;
    while i < n {
        let c = bytes[i];
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'\'' || c == b'"' {
            in_quote = Some(c);
            i += 1;
            continue;
        }
        if i + 7 <= n
            && text
                .get(i..i + 7)
                .is_some_and(|s| s.eq_ignore_ascii_case("project"))
        {
            let before_ok = i == 0 || !meson_ident_byte(bytes[i - 1]);
            if before_ok {
                let mut j = i + 7;
                while j < n && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < n && bytes[j] == b'(' {
                    return Some(j);
                }
            }
        }
        i += 1;
    }
    None
}

fn meson_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn meson_version_in_call(search: &str) -> Option<String> {
    let mut search = search;
    while let Some(idx) = search.to_ascii_lowercase().find("version") {
        let before = search[..idx].trim_end();
        if before.to_ascii_lowercase().ends_with("meson_") {
            search = &search[idx + "version".len()..];
            continue;
        }
        let after = search[idx + "version".len()..].trim_start();
        let Some(after) = after.strip_prefix(':') else {
            search = &search[idx + "version".len()..];
            continue;
        };
        let after = after.trim_start();
        let quote = after.chars().next().filter(|c| *c == '\'' || *c == '"')?;
        let inner = after.get(1..)?.find(quote)?;
        return Some(after[1..1 + inner].to_string());
    }
    None
}

/// Index of the `)` that closes the `(` at `open`, ignoring parens inside
/// `'...'` / `"..."` and `#` comments.
fn meson_matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = open + 1;
    let mut depth = 1usize;
    let mut in_quote: Option<u8> = None;
    let mut escaped = false;
    while i < n && depth > 0 {
        let c = bytes[i];
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'\'' || c == b'"' {
            in_quote = Some(c);
            i += 1;
            continue;
        }
        if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `AC_INIT([name], [1.2.3], ...)`.
fn autoconf_init_version(text: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?is)AC_INIT\s*\(\s*\[[^\]]*\]\s*,\s*\[([^\]]+)\]")
            .expect("literal autoconf init")
    });
    Some(re.captures(text)?.get(1)?.as_str().trim().to_string())
}

/// The part of an easyconfig version that names a release, with any snapshot
/// marker removed: `4.3.9-20260811` and `4.3.9-dev_20260811` both give `4.3.9`.
fn release_part(recipe_version: &str) -> &str {
    let re = regex::Regex::new(r"^(.*?)[-_.](?:dev_?)?(?:20\d{6}|[0-9a-f]{7,40})$").ok();
    match re.and_then(|r| {
        r.captures(recipe_version)
            .and_then(|c| c.get(1))
            .map(|m| m.start()..m.end())
    }) {
        Some(range) => &recipe_version[range],
        None => recipe_version,
    }
}

/// Compare the version a recipe carries against the one its source declares.
///
/// This answers a question only a source tree can: a recipe pinned to a commit
/// is free to call itself anything, and naming it after the last tag is the
/// mistake that reads as correct.
pub fn verify_declared_version(
    recipe_version: &str,
    declared: Option<&DeclaredVersion>,
) -> Vec<SourceFinding> {
    let mut findings = Vec::new();
    let Some(declared) = declared else {
        findings.push(SourceFinding::warning(
            "no build system in the source tree declares a version, so the recipe version              is unverified against what the code reports"
                .into(),
        ));
        return findings;
    };
    let release = release_part(recipe_version);
    if release != declared.value {
        findings.push(SourceFinding::warning(format!(
            "recipe version {recipe_version} names release {release}, while {} declares {}; \
             a module built from this source reports {} to whoever runs it",
            declared.source, declared.value, declared.value
        )));
    }
    findings
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
    fn a_github_tag_archive_url_wins_over_a_git_remote() {
        let archive = "https://github.com/acct/Name/archive/v1.0.tar.gz";
        let git = "https://github.com/acct/Name.git";
        assert_eq!(
            classify_foreign(Some(archive), Some(git)),
            ArtifactClass::GitHubTagArchive
        );
        let seed = SeededChecksum {
            origin: "spack".into(),
            source_url: Some(archive.into()),
            git: Some(git.into()),
            sha256: Some("a".repeat(64)),
        };
        let findings = verify_sources(&[archive.into()], Some(&seed));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_github_release_asset_url_wins_over_a_git_remote() {
        let asset =
            "https://github.com/TheochemUI/eOn/releases/download/v2.16.0/eon-v2.16.0.tar.xz";
        let git = "https://github.com/TheochemUI/eOn.git";
        assert_eq!(
            classify_foreign(Some(asset), Some(git)),
            ArtifactClass::GitHubReleaseAsset
        );
        let seed = SeededChecksum {
            origin: "spack".into(),
            source_url: Some(asset.into()),
            git: Some(git.into()),
            sha256: Some("a".repeat(64)),
        };
        let findings = verify_sources(&[asset.into()], Some(&seed));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_pypi_sdist_url_wins_over_a_git_remote() {
        let sdist = "https://pypi.org/packages/source/f/foo/foo-1.0.tar.gz";
        let git = "https://github.com/foo/foo.git";
        assert_eq!(
            classify_foreign(Some(sdist), Some(git)),
            ArtifactClass::PyPiSdist
        );
        let seed = SeededChecksum {
            origin: "spack".into(),
            source_url: Some(sdist.into()),
            git: Some(git.into()),
            sha256: Some("a".repeat(64)),
        };
        let findings = verify_sources(&[sdist.into()], Some(&seed));
        assert!(findings.is_empty(), "{findings:?}");
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

#[cfg(test)]
mod declared_version_tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        dir
    }

    #[test]
    fn toml_package_version_strips_comments_and_refuses_workspace_inherit() {
        assert_eq!(
            toml_package_version("[package]\nversion = \"0.13.1\" # release\n"),
            Some("0.13.1".into())
        );
        assert_eq!(
            toml_package_version("[package]\nversion = { workspace = true }\n"),
            None
        );
    }

    #[test]
    fn cmake_project_version_skips_comments_and_keeps_description_parens() {
        assert_eq!(
            cmake_project_version("# see project()\nproject(Foo VERSION 4.3.9)\n"),
            Some("4.3.9".into())
        );
        assert_eq!(
            cmake_project_version("project(Foo DESCRIPTION \"Fast (FFT)\" VERSION 4.3.9)\n"),
            Some("4.3.9".into())
        );
        assert_eq!(
            cmake_project_version(
                "project(Foo DESCRIPTION \"Requires VERSION 2.0 API\" VERSION 4.3.9)\n"
            ),
            Some("4.3.9".into())
        );
    }

    #[test]
    fn meson_project_version_stays_inside_the_project_call() {
        assert_eq!(
            meson_project_version(
                "project('foo', 'c', meson_version: '>=1.8.0')\ndependency('bar', version: '>=1.2.3')\n"
            ),
            None
        );
    }

    #[test]
    fn meson_project_version_skips_subproject_and_allows_whitespace() {
        assert_eq!(
            meson_project_version(
                "subproject('wrap', version: '1.2.3')\nproject('x', 'c', version: '4.0.0')\n"
            ),
            Some("4.0.0".into())
        );
        assert_eq!(
            meson_project_version("project (\n  'x',\n  'c',\n  version: '1.4.2',\n)\n"),
            Some("1.4.2".into())
        );
        assert_eq!(
            meson_project_version(
                "# project('old', version: '0.1')\nproject('x', version: '2.0')\n"
            ),
            Some("2.0".into())
        );
    }

    #[test]
    fn reads_a_cmake_project_version_across_lines() {
        let dir = tree(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.21)\nproject(\n  example\n  VERSION 4.3.9\n  LANGUAGES C CXX)\n",
        )]);
        let found = declared_version(dir.path()).unwrap();
        assert_eq!(found.value, "4.3.9");
        assert_eq!(found.source, "CMakeLists.txt");
    }

    #[test]
    fn reads_cargo_pyproject_meson_and_autoconf() {
        for (name, body, want) in [
            (
                "Cargo.toml",
                "[package]\nname = \"x\"\nversion = \"0.13.1\"\n",
                "0.13.1",
            ),
            (
                "pyproject.toml",
                "[project]\nname = \"x\"\nversion = \"2.1.0\"\n",
                "2.1.0",
            ),
            (
                "meson.build",
                "project('x', 'c', version : '1.4.2')\n",
                "1.4.2",
            ),
            (
                "configure.ac",
                "AC_INIT([x], [3.0.1], [bugs@example])\n",
                "3.0.1",
            ),
        ] {
            let dir = tree(&[(name, body)]);
            let found = declared_version(dir.path()).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(found.value, want, "{name}");
        }
    }

    #[test]
    fn a_tree_that_declares_nothing_is_reported_as_unverified() {
        let dir = tree(&[("README.md", "nothing here\n")]);
        assert!(declared_version(dir.path()).is_none());
        let findings = verify_declared_version("1.0.0", None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].level, FindingLevel::Warning);
    }

    /// The real case: a snapshot named after the last release tag while the
    /// source had already moved on.
    #[test]
    fn flags_a_snapshot_named_after_the_wrong_release() {
        let declared = DeclaredVersion {
            value: "4.3.9".into(),
            source: "CMakeLists.txt".into(),
        };
        let findings = verify_declared_version("4.3.0-20260811", Some(&declared));
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].message.contains("4.3.9"), "{findings:?}");
    }

    #[test]
    fn a_snapshot_named_after_what_the_source_declares_is_quiet() {
        let declared = DeclaredVersion {
            value: "4.3.9".into(),
            source: "CMakeLists.txt".into(),
        };
        for version in [
            "4.3.9-20260811",
            "4.3.9-dev_20260811",
            "4.3.9-b9eb286",
            "4.3.9",
        ] {
            assert!(
                verify_declared_version(version, Some(&declared)).is_empty(),
                "{version}"
            );
        }
    }

    #[test]
    fn a_release_build_is_checked_the_same_way() {
        let declared = DeclaredVersion {
            value: "4.3.0".into(),
            source: "CMakeLists.txt".into(),
        };
        assert!(verify_declared_version("4.3.0", Some(&declared)).is_empty());
        assert_eq!(verify_declared_version("4.2.0", Some(&declared)).len(), 1);
    }
}
