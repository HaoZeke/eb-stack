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

/// A class an easyblock module defines at top level.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EasyblockClass {
    /// Class name, e.g. `EB_SeisSol`.
    pub name: String,
    /// Base classes as written, e.g. `CMakeMake`.
    pub bases: Vec<String>,
    /// Easyconfig parameters the class adds in `extra_options`, in source order.
    pub extra_options: Vec<String>,
}

/// Classes an easyblock module defines, read from its syntax tree.
///
/// EasyBuild imports the module and looks a class up by name, so only
/// top-level `class` statements count. The file is parsed, never executed.
/// `extra_options` keys are the string keys of the dict literals in that
/// method's body, which is how every easyblock in the framework declares them.
pub fn defined_classes(source: &str, path: &str) -> Result<Vec<EasyblockClass>, String> {
    use rustpython_parser::ast;
    use rustpython_parser::Parse;

    let suite = ast::Suite::parse(source, path).map_err(|error| error.to_string())?;
    Ok(suite
        .iter()
        .filter_map(|statement| match statement {
            ast::Stmt::ClassDef(class) => Some(class),
            _ => None,
        })
        .map(|class| EasyblockClass {
            name: class.name.to_string(),
            bases: class.bases.iter().filter_map(dotted_name).collect(),
            extra_options: class
                .body
                .iter()
                .filter_map(|statement| match statement {
                    ast::Stmt::FunctionDef(function) if function.name.as_str() == "extra_options" => {
                        Some(function)
                    }
                    _ => None,
                })
                .flat_map(|function| {
                    let mut keys = Vec::new();
                    collect_dict_keys(&function.body, &mut keys);
                    keys
                })
                .collect(),
        })
        .collect())
}

fn dotted_name(expression: &rustpython_parser::ast::Expr) -> Option<String> {
    use rustpython_parser::ast;
    match expression {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attribute) => {
            dotted_name(&attribute.value).map(|base| format!("{base}.{}", attribute.attr))
        }
        _ => None,
    }
}

fn collect_dict_keys(statements: &[rustpython_parser::ast::Stmt], keys: &mut Vec<String>) {
    use rustpython_parser::ast;
    fn from_expression(expression: &ast::Expr, keys: &mut Vec<String>) {
        match expression {
            ast::Expr::Dict(dict) => {
                for key in dict.keys.iter().flatten() {
                    if let ast::Expr::Constant(ast::ExprConstant {
                        value: ast::Constant::Str(key),
                        ..
                    }) = key
                    {
                        if !keys.contains(key) {
                            keys.push(key.clone());
                        }
                    }
                }
            }
            ast::Expr::Call(call) => {
                for argument in &call.args {
                    from_expression(argument, keys);
                }
            }
            _ => {}
        }
    }
    for statement in statements {
        match statement {
            ast::Stmt::Assign(assign) => from_expression(&assign.value, keys),
            ast::Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    from_expression(value, keys);
                }
            }
            ast::Stmt::Expr(expression) => from_expression(&expression.value, keys),
            ast::Stmt::Return(ret) => {
                if let Some(value) = &ret.value {
                    from_expression(value, keys);
                }
            }
            ast::Stmt::If(branch) => {
                collect_dict_keys(&branch.body, keys);
                collect_dict_keys(&branch.orelse, keys);
            }
            _ => {}
        }
    }
}

/// Where each easyblock class is defined, across trees of `.py` files.
///
/// Roots are searched in order and the first definition of a name wins, which
/// is the precedence `--include-easyblocks` gives files over the installed
/// package. A file that does not parse is reported rather than skipped: an
/// easyblock EasyBuild cannot import is a build failure waiting to happen.
pub fn index_easyblocks(
    roots: &[&std::path::Path],
) -> Result<std::collections::BTreeMap<String, (std::path::PathBuf, EasyblockClass)>, String> {
    let mut files = Vec::new();
    for root in roots {
        collect_python_files(root, &mut files)
            .map_err(|error| format!("read easyblocks {}: {error}", root.display()))?;
    }
    let mut index = std::collections::BTreeMap::new();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|error| format!("read easyblock {}: {error}", file.display()))?;
        let classes = defined_classes(&source, &file.display().to_string())
            .map_err(|error| format!("parse easyblock {}: {error}", file.display()))?;
        for class in classes {
            index
                .entry(class.name.clone())
                .or_insert_with(|| (file.clone(), class));
        }
    }
    Ok(index)
}

fn collect_python_files(
    root: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }
    let mut entries = std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "__pycache__") {
                continue;
            }
            collect_python_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "py")
            && path.file_name().is_some_and(|name| name != "__init__.py")
        {
            files.push(path);
        }
    }
    Ok(())
}

/// The directory EasyBuild's own easyblocks package keeps a module in.
///
/// `easybuild/easyblocks/<letter>/<module>.py`, with the letter taken from the
/// module filename; `--include-easyblocks` does not need the layout, but an
/// easyblocks PR does, and a bundle that already has it can be diffed against one.
pub fn easyblock_letter_dir(filename: &str) -> String {
    filename
        .chars()
        .next()
        .map(|first| first.to_ascii_lowercase().to_string())
        .filter(|letter| letter.chars().all(|c| c.is_ascii_lowercase()))
        .unwrap_or_else(|| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEISSOL_LIKE: &str = r#"
from easybuild.easyblocks.generic.cmakemake import CMakeMake
from easybuild.framework.easyconfig import CUSTOM

class EB_SeisSol(CMakeMake):
    """Derive HOST_ARCH."""

    @staticmethod
    def extra_options():
        extra_vars = {
            'host_arch': ['auto', "HOST_ARCH", CUSTOM],
            'order': [6, "ORDER", CUSTOM],
        }
        if True:
            extra_vars.update({'equations': ['elastic', "EQUATIONS", CUSTOM]})
        return CMakeMake.extra_options(extra_vars)

    def configure_step(self):
        local = {'not_an_option': 1}

class _Helper(object):
    pass
"#;

    #[test]
    fn top_level_classes_and_their_bases() {
        let classes = defined_classes(SEISSOL_LIKE, "seissol.py").unwrap();
        let names: Vec<_> = classes.iter().map(|class| class.name.as_str()).collect();
        assert_eq!(names, ["EB_SeisSol", "_Helper"]);
        assert_eq!(classes[0].bases, ["CMakeMake"]);
        assert_eq!(classes[1].bases, ["object"]);
    }

    #[test]
    fn extra_options_come_from_that_method_only() {
        let classes = defined_classes(SEISSOL_LIKE, "seissol.py").unwrap();
        assert_eq!(classes[0].extra_options, ["host_arch", "order", "equations"]);
        assert!(classes[1].extra_options.is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_is_an_error() {
        assert!(defined_classes("class EB_X(:\n", "x.py").is_err());
    }

    #[test]
    fn easyblock_modules_sit_under_their_first_letter() {
        assert_eq!(easyblock_letter_dir("seissol.py"), "s");
        assert_eq!(easyblock_letter_dir("GROMACS.py"), "g");
        assert_eq!(easyblock_letter_dir("_generic.py"), "0");
    }

    #[test]
    fn the_first_root_wins_a_class_name() {
        let base = std::env::temp_dir().join(format!("eb-stack-easyblocks-{}", std::process::id()));
        let first = base.join("bundle/s");
        let second = base.join("installed/s");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(second.join("__pycache__")).unwrap();
        std::fs::write(first.join("seissol.py"), SEISSOL_LIKE).unwrap();
        std::fs::write(second.join("seissol.py"), "class EB_SeisSol(object):\n    pass\n").unwrap();
        std::fs::write(second.join("__pycache__/junk.py"), "not python (").unwrap();
        let index = index_easyblocks(&[&base.join("bundle"), &base.join("installed")]).unwrap();
        let (path, class) = &index["EB_SeisSol"];
        assert!(path.starts_with(base.join("bundle")));
        assert_eq!(class.bases, ["CMakeMake"]);
        std::fs::remove_dir_all(&base).unwrap();
    }

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
