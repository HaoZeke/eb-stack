//! Which easyblock class EasyBuild will look for, given a software name.
//!
//! An easyconfig that names no `easyblock` is not asking for a default. There
//! is no default: EasyBuild derives a class name from the software name, tries
//! to import it, and stops with `No software-specific easyblock '<class>' found
//! for <name>` when it cannot. So the derived name is the difference between a
//! recipe that installs and a recipe that refuses to start, and it is worth
//! being able to compute without a Python interpreter to hand.
//!
//! The rule mirrors `easybuild.tools.filetools`: `encode_class_name` is
//! `EASYBLOCK_CLASS_PREFIX` followed by `encode_string`, and `encode_string`
//! substitutes each character through a fixed map, passing anything unmapped
//! through unchanged.
//!
//! Two properties of that map are easy to get wrong. `_` is itself escaped, to
//! `_underscore_`, which is what keeps the encoding reversible. And the
//! substitution is per character with no collapsing, so a doubled separator
//! doubles: `c++` becomes `EB_c_plus__plus_`.

/// Prefix EasyBuild puts on a derived easyblock class name.
///
/// `EASYBLOCK_CLASS_PREFIX` in `easybuild.tools.filetools`.
pub const EASYBLOCK_CLASS_PREFIX: &str = "EB_";

/// Per-character substitutions applied to a software name.
///
/// `STRING_ENCODING_CHARMAP` in `easybuild.tools.filetools`. Characters absent
/// from this table are passed through, so letters and digits survive intact.
pub const STRING_ENCODING_CHARMAP: &[(char, &str)] = &[
    (' ', "_space_"),
    ('!', "_exclamation_"),
    ('"', "_quotation_"),
    ('#', "_hash_"),
    ('$', "_dollar_"),
    ('%', "_percent_"),
    ('&', "_ampersand_"),
    ('(', "_leftparen_"),
    (')', "_rightparen_"),
    ('*', "_asterisk_"),
    ('+', "_plus_"),
    (',', "_comma_"),
    ('-', "_minus_"),
    ('.', "_period_"),
    ('/', "_slash_"),
    (':', "_colon_"),
    (';', "_semicolon_"),
    ('<', "_lessthan_"),
    ('=', "_equals_"),
    ('>', "_greaterthan_"),
    ('?', "_question_"),
    ('@', "_atsign_"),
    ('[', "_leftbracket_"),
    ('\'', "_apostrophe_"),
    ('\\', "_backslash_"),
    (']', "_rightbracket_"),
    ('^', "_circumflex_"),
    ('_', "_underscore_"),
    ('`', "_backquote_"),
    ('{', "_leftcurly_"),
    ('|', "_verticalbar_"),
    ('}', "_rightcurly_"),
    ('~', "_tilde_"),
];

/// How many substitutions the table carries, for a coverage assertion.
pub const STRING_ENCODING_CHARMAP_COUNT: usize = 33;

/// Substitute a software name through the encoding map.
///
/// `encode_string` in `easybuild.tools.filetools`.
pub fn encode_string(name: &str) -> String {
    name.chars()
        .map(|c| {
            STRING_ENCODING_CHARMAP
                .iter()
                .find(|(from, _)| *from == c)
                .map(|(_, to)| (*to).to_string())
                .unwrap_or_else(|| c.to_string())
        })
        .collect()
}

/// The easyblock class name EasyBuild derives for `name`.
///
/// This says what EasyBuild will *look for*, not that it exists. A name that
/// has no such class is the case where EasyBuild stops rather than falling back
/// to a generic easyblock.
pub fn encode_class_name(name: &str) -> String {
    format!("{EASYBLOCK_CLASS_PREFIX}{}", encode_string(name))
}

/// Recover the software name from an encoded class name.
///
/// `decode_class_name` in `easybuild.tools.filetools`. A name without the
/// prefix is returned unchanged, since it was never encoded.
pub fn decode_class_name(class_name: &str) -> String {
    match class_name.strip_prefix(EASYBLOCK_CLASS_PREFIX) {
        None => class_name.to_string(),
        Some(rest) => decode_string(rest),
    }
}

/// Reverse the encoding map.
///
/// Longest-match first, because `_underscore_` and `_` share a prefix and
/// taking the shorter one first would strand the rest of the token.
pub fn decode_string(encoded: &str) -> String {
    let mut by_length: Vec<&(char, &str)> = STRING_ENCODING_CHARMAP.iter().collect();
    by_length.sort_by_key(|(_, to)| std::cmp::Reverse(to.len()));

    let mut out = String::with_capacity(encoded.len());
    let mut rest = encoded;
    'outer: while !rest.is_empty() {
        for (from, to) in &by_length {
            if let Some(tail) = rest.strip_prefix(*to) {
                out.push(*from);
                rest = tail;
                continue 'outer;
            }
        }
        let mut chars = rest.chars();
        // Safe: rest is non-empty, so next() yields.
        if let Some(c) = chars.next() {
            out.push(c);
        }
        rest = chars.as_str();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charmap_is_complete() {
        assert_eq!(STRING_ENCODING_CHARMAP.len(), STRING_ENCODING_CHARMAP_COUNT);
    }

    #[test]
    fn charmap_has_no_duplicate_sources() {
        let mut seen: Vec<char> = STRING_ENCODING_CHARMAP.iter().map(|(c, _)| *c).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "a character is mapped twice");
    }

    #[test]
    fn hyphen_becomes_minus() {
        // The case that sends people looking for a file called code-server.py.
        assert_eq!(encode_class_name("code-server"), "EB_code_minus_server");
    }

    #[test]
    fn letters_and_digits_pass_through() {
        assert_eq!(encode_class_name("GROMACS"), "EB_GROMACS");
        assert_eq!(encode_class_name("Qt6"), "EB_Qt6");
    }

    #[test]
    fn a_doubled_separator_doubles() {
        // Per character, with no collapsing. The framework docstring names c++
        // and C# as the reason the escaping exists at all.
        assert_eq!(encode_class_name("c++"), "EB_c_plus__plus_");
        assert_eq!(encode_class_name("C#"), "EB_C_hash_");
    }

    #[test]
    fn underscore_is_itself_escaped() {
        // The property that keeps the mapping reversible, and the one most
        // easily missed: an underscore in the name is not left alone.
        assert_eq!(encode_string("a_b"), "a_underscore_b");
    }

    #[test]
    fn matches_the_framework_docstring_example() {
        // encode_string's own docstring in easybuild.tools.filetools.
        assert_eq!(
            encode_string("0_foo+0x0x#-$__"),
            "0_underscore_foo_plus_0x0x_hash__minus__dollar__underscore__underscore_"
        );
    }

    #[test]
    fn decoding_recovers_the_name() {
        for name in [
            "code-server",
            "GROMACS",
            "c++",
            "C#",
            "a_b",
            "0_foo+0x0x#-$__",
            "Xerces-C++",
        ] {
            assert_eq!(
                decode_class_name(&encode_class_name(name)),
                name,
                "round trip failed for {name}"
            );
        }
    }

    #[test]
    fn an_unencoded_class_name_is_left_alone() {
        // decode_class_name's documented behaviour: no prefix means it was
        // never encoded, so it is not this function's business.
        assert_eq!(decode_class_name("ConfigureMake"), "ConfigureMake");
        assert_eq!(decode_class_name("PythonBundle"), "PythonBundle");
    }
}
