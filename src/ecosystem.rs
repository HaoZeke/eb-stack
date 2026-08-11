//! Dependency-spec helpers shared by the language-ecosystem parsers.
//!
//! PyPI, CRAN and every ecosystem after them write a dependency the same way:
//! a name, then an optional comparator and version. The grammars differ in what
//! surrounds that pair (PEP 508 markers and extras, R's parenthesised pins),
//! and that part belongs in the ecosystem module. The split itself does not,
//! and keeping one copy is what stops the two implementations drifting: the
//! PyPI copy grew parenthesis handling it never needed while the CRAN copy,
//! whose grammar is the one that uses parentheses, lost it.

/// Split `name >= 1.2` into its name and its pin.
///
/// Surrounding parentheses are stripped, so both `jsonlite (>= 1.8.0)` and the
/// parenthesised PEP 508 form reduce to the same pair. The pin keeps its
/// comparator, since only the caller knows which comparators its grammar
/// allows. Returns `None` for the pin when the spec is a bare name.
pub(crate) fn split_name_and_pin(spec: &str) -> (String, Option<String>) {
    let spec = spec.trim();
    // `(` cuts as well as the comparators, because R writes the whole pin
    // inside parentheses and PEP 508 permits the same form.
    let cut = spec
        .char_indices()
        .find(|(_, character)| matches!(character, '<' | '>' | '=' | '!' | '~' | '('))
        .map_or(spec.len(), |(index, _)| index);
    let name = spec[..cut].trim().to_string();
    let pin = spec[cut..]
        .trim()
        .trim_matches(|c| c == '(' || c == ')')
        .trim();
    let pin = (!pin.is_empty()).then(|| pin.to_string());
    (name, pin)
}

/// The version an `==` pin names, if the pin is an exact one.
///
/// Anything else, including `>=` and a range, is a bound rather than a version
/// and cannot become an EasyBuild dependency version on its own.
pub(crate) fn exact_version(pin: &str) -> Option<String> {
    pin.trim()
        .strip_prefix("==")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// `Some(trimmed)` when a metadata field carries anything but whitespace.
///
/// Foreign metadata routinely writes an empty string where a field is absent,
/// and an empty homepage or summary should not reach an emitted recipe.
pub(crate) fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

/// One package as a repository index describes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexEntry {
    /// Version the index publishes.
    pub version: String,
    /// Checksum the index publishes, already tagged with its algorithm so it
    /// can go straight into an easyconfig (`md5:...`).
    pub checksum: Option<String>,
}

/// Parse a repository index into package name to version.
///
/// The format is the one CRAN publishes as `PACKAGES` and Debian calls a
/// control file: stanzas of `Field: value` separated by blank lines. It is the
/// artifact a site already has offline, which is what an overlay needs when a
/// dependency states no version of its own.
pub fn parse_package_index(text: &str) -> std::collections::BTreeMap<String, IndexEntry> {
    let mut index = std::collections::BTreeMap::new();
    for stanza in text.split("\n\n") {
        let mut name = None;
        let mut version = None;
        let mut checksum = None;
        for line in stanza.lines() {
            if let Some(rest) = line.strip_prefix("Package:") {
                name = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("Version:") {
                version = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("MD5sum:") {
                let digest = rest.trim();
                if !digest.is_empty() {
                    checksum = Some(format!("md5:{digest}"));
                }
            }
        }
        if let (Some(name), Some(version)) = (name, version) {
            index.insert(
                crate::package_sources::package_identity(&name),
                IndexEntry { version, checksum },
            );
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_has_no_pin() {
        assert_eq!(split_name_and_pin("jsonlite"), ("jsonlite".into(), None));
        assert_eq!(
            split_name_and_pin("  beautifulsoup4  "),
            ("beautifulsoup4".into(), None)
        );
    }

    #[test]
    fn both_grammars_split_the_same_way() {
        // R writes the pin in parentheses, PEP 508 usually does not, and the
        // caller should not have to care which one it handed over.
        assert_eq!(
            split_name_and_pin("jsonlite (>= 1.8.0)"),
            ("jsonlite".into(), Some(">= 1.8.0".into()))
        );
        assert_eq!(
            split_name_and_pin("soupsieve>1.2"),
            ("soupsieve".into(), Some(">1.2".into()))
        );
        assert_eq!(
            split_name_and_pin("numpy==2.1.3"),
            ("numpy".into(), Some("==2.1.3".into()))
        );
    }

    #[test]
    fn only_an_equality_pin_is_a_version() {
        assert_eq!(exact_version("==2.1.3"), Some("2.1.3".into()));
        assert_eq!(exact_version("  ==  2.1.3 "), Some("2.1.3".into()));
        assert_eq!(exact_version(">=2.1.3"), None);
        assert_eq!(exact_version("=="), None);
    }

    #[test]
    fn an_index_maps_every_stanza() {
        let index = parse_package_index(
            "Package: matrixStats\nVersion: 1.4.1\nMD5sum: abc123\n\nPackage: coda\nVersion: 0.19-4\n",
        );
        let entry = index.get("matrixstats").expect("matrixstats");
        assert_eq!(entry.version, "1.4.1");
        assert_eq!(
            index.get("coda").map(|e| e.version.as_str()),
            Some("0.19-4")
        );
        assert_eq!(index.len(), 2);
        assert_eq!(entry.checksum.as_deref(), Some("md5:abc123"));
        assert_eq!(index.get("coda").and_then(|e| e.checksum.as_deref()), None);
    }

    #[test]
    fn blank_metadata_fields_are_absent() {
        assert_eq!(nonempty(Some("  ".into())), None);
        assert_eq!(nonempty(None), None);
        assert_eq!(
            nonempty(Some(" https://x ".into())),
            Some("https://x".into())
        );
    }
}
