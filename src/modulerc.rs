//! Lmod module version aliases.
//!
//! A module tree's `modules/all/<Name>/.modulerc.lua` publishes short versions
//! with `module_version("Name/full", "alias")`. `module load Name/alias` loads
//! `Name/full`. EasyBuild treats that alias as the installed module, which is
//! why `Java/21` is installed when the tree has `Java/21.0.8` and a modulerc
//! that aliases it to `21`.

use std::collections::BTreeSet;
use std::path::Path;

/// One `module_version` alias: `full` is `Name/version`, `alias` is `Name/short`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleVersionAlias {
    /// The module the alias points at, e.g. `Java/21.0.8`.
    pub full: String,
    /// The name Lmod also answers to, e.g. `Java/21`.
    pub alias: String,
}

/// Read `module_version("Name/full", "alias"[, ...])` calls from a modulerc.
///
/// Lua line comments are ignored. A call may span lines. Each alias after the
/// full module name becomes its own `Name/alias` when the alias has no `/`.
pub fn parse_modulerc(text: &str) -> Vec<ModuleVersionAlias> {
    let cleaned = strip_lua_comments(text);
    let mut out = Vec::new();
    let bytes = cleaned.as_bytes();
    let mut index = 0;
    let needle = b"module_version";
    while index < bytes.len() {
        if cleaned[index..].starts_with("module_version")
            && ident_boundary(bytes, index, needle.len())
        {
            index += needle.len();
            index = skip_ws(&cleaned, index);
            if index < bytes.len() && bytes[index] == b'(' {
                index += 1;
                let (args, next) = parse_string_args(&cleaned, index);
                index = next;
                if let Some((full, aliases)) = args.split_first() {
                    for alias in aliases {
                        if let Some(name) = alias_module(full, alias) {
                            out.push(ModuleVersionAlias {
                                full: full.clone(),
                                alias: name,
                            });
                        }
                    }
                }
            }
        } else {
            index += cleaned[index..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        }
    }
    out
}

/// Add an alias to the installed set when the module it points at is installed.
pub fn apply_modulerc(
    installed: &BTreeSet<String>,
    aliases: &[ModuleVersionAlias],
) -> BTreeSet<String> {
    let mut out = installed.clone();
    for alias in aliases {
        if installed.contains(&alias.full) {
            out.insert(alias.alias.clone());
        }
    }
    out
}

/// Read a modulerc file, or every `.modulerc.lua` under a module tree.
pub fn load_modulerc(path: &Path) -> Result<Vec<ModuleVersionAlias>, String> {
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read modulerc {}: {error}", path.display()))?;
        return Ok(parse_modulerc(&text));
    }
    if !path.is_dir() {
        return Err(format!(
            "modulerc path {} is not a file or directory",
            path.display()
        ));
    }
    let mut files = Vec::new();
    collect_modulercs(path, &mut files)
        .map_err(|error| format!("read modulerc {}: {error}", path.display()))?;
    files.sort();
    let mut aliases = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|error| format!("read modulerc {}: {error}", file.display()))?;
        aliases.extend(parse_modulerc(&text));
    }
    Ok(aliases)
}

fn collect_modulercs(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_modulercs(&path, files)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some(".modulerc.lua") {
            files.push(path);
        }
    }
    Ok(())
}

fn alias_module(full: &str, alias: &str) -> Option<String> {
    if alias.is_empty() || full.is_empty() {
        return None;
    }
    if alias.contains('/') {
        return Some(alias.to_string());
    }
    let (name, _) = full.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    Some(format!("{name}/{alias}"))
}

fn ident_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
    let after = start + len;
    let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
    before_ok && after_ok
}

fn skip_ws(text: &str, mut index: usize) -> usize {
    while index < text.len() && text.as_bytes()[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

/// Quoted string arguments of a call, then the index just after the call.
fn parse_string_args(text: &str, mut index: usize) -> (Vec<String>, usize) {
    let bytes = text.as_bytes();
    let mut args = Vec::new();
    while index < bytes.len() && bytes[index] != b')' {
        index = skip_ws(text, index);
        if index >= bytes.len() || bytes[index] == b')' {
            break;
        }
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index] != quote {
                index += 1;
            }
            args.push(text[start..index].to_string());
            if index < bytes.len() {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    if index < bytes.len() && bytes[index] == b')' {
        index += 1;
    }
    (args, index)
}

fn strip_lua_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(&strip_line_comment(line));
        out.push('\n');
    }
    out
}

fn strip_line_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if byte == b'-' && bytes[index + 1] == b'-' {
            return line[..index].to_string();
        }
        index += 1;
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_version_and_default_alias_the_full_module() {
        let text = r#"
-- module_version("Nope/1", "9")
module_version("Java/21.0.8", "21")
module_version(
    "Java/21.0.8",
    "default"
)
"#;
        let aliases = parse_modulerc(text);
        assert_eq!(
            aliases,
            vec![
                ModuleVersionAlias {
                    full: "Java/21.0.8".into(),
                    alias: "Java/21".into(),
                },
                ModuleVersionAlias {
                    full: "Java/21.0.8".into(),
                    alias: "Java/default".into(),
                },
            ]
        );
    }

    #[test]
    fn an_alias_counts_only_when_the_full_module_is_installed() {
        let aliases = parse_modulerc(r#"module_version("Java/21.0.8", "21", "default")"#);
        let installed = BTreeSet::from(["Java/21.0.8".to_string()]);
        let with = apply_modulerc(&installed, &aliases);
        assert!(with.contains("Java/21"));
        assert!(with.contains("Java/default"));

        let missing = BTreeSet::from(["Java/17.0.1".to_string()]);
        let without = apply_modulerc(&missing, &aliases);
        assert!(!without.contains("Java/21"));
    }
}
