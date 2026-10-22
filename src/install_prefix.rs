//! Facts read from an installed EESSI prefix.
//!
//! The install directory of a module `Name/version` is
//! `<prefix>/software/Name/version`. A static archive is a `*.a` file
//! directly under a `lib*` directory there (`lib/libz.a`, `lib64/libz.a`).
//!
//! `--check-static` reads defined dynamic symbols, the names `nm -D
//! --defined-only` prints. The patched library's names come from its
//! shared objects, or from the archive when it ships no shared object.
//! A review dependent that defines one of those names must be rebuilt.
//! A dependent that defines none is no-rebuild.

use crate::rebuild_set::{
    RebuildError, RebuildRow, CLASS_MUST_REBUILD, CLASS_NO_REBUILD, CLASS_REVIEW,
};
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

enum ArchiveFact {
    Missing,
    Names(Vec<String>),
}

/// Reclassify builddependency-only rows from the static archives on disk.
///
/// A patched module whose install directory exists and contains no `*.a`
/// cannot have been linked statically. Those rows become no-rebuild. A
/// module that does ship archives stays in review, and the reason names
/// the archives. A missing install directory is a note; the row stays.
pub fn apply_static_archives(
    rows: &mut [RebuildRow],
    prefix: &Path,
    notes: &mut BTreeSet<String>,
) -> Result<(), RebuildError> {
    let software = prefix.join("software");
    if !software.is_dir() {
        return Err(RebuildError::Prefix {
            path: prefix.display().to_string(),
        });
    }
    let mut modules_for: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows.iter() {
        if row.edge_kind == "changed" {
            modules_for
                .entry(row.changed.clone())
                .or_default()
                .push(row.dependent.clone());
        }
    }
    let mut facts: BTreeMap<String, ArchiveFact> = BTreeMap::new();
    for (file, modules) in &modules_for {
        let mut names = BTreeSet::new();
        let mut saw_dir = false;
        for module in modules {
            match archives_in(&software, module) {
                ArchiveFact::Missing => {
                    notes.insert(format!("{module}: install directory not found"));
                }
                ArchiveFact::Names(found) => {
                    saw_dir = true;
                    names.extend(found);
                }
            }
        }
        facts.insert(
            file.clone(),
            if saw_dir {
                ArchiveFact::Names(names.into_iter().collect())
            } else {
                ArchiveFact::Missing
            },
        );
    }
    for row in rows.iter_mut() {
        if row.class != CLASS_REVIEW {
            continue;
        }
        let Some(fact) = facts.get(&row.changed) else {
            continue;
        };
        match fact {
            ArchiveFact::Missing => {}
            ArchiveFact::Names(names) if names.is_empty() => {
                row.class = CLASS_NO_REBUILD.to_string();
                row.reason = "no static archive installed".to_string();
            }
            ArchiveFact::Names(names) => {
                row.reason = reason_with_archives(&row.reason, names);
            }
        }
    }
    Ok(())
}

fn archives_in(software: &Path, module: &str) -> ArchiveFact {
    let dir = software.join(module);
    if !dir.is_dir() {
        return ArchiveFact::Missing;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return ArchiveFact::Names(Vec::new());
    };
    let mut children: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    children.sort_by_key(|entry| entry.file_name());
    let mut names = BTreeSet::new();
    for entry in children {
        let dir_name = entry.file_name();
        let dir_name = dir_name.to_string_lossy();
        if !dir_name.starts_with("lib") {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut archives: Vec<_> = files.filter_map(|file| file.ok()).collect();
        archives.sort_by_key(|file| file.file_name());
        for file in archives {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".a") && file.path().is_file() {
                names.insert(name.into_owned());
            }
        }
    }
    ArchiveFact::Names(names.into_iter().collect())
}

const SAMPLE: usize = 8;
const SKIP_DIRS: &[&str] = &[
    "include",
    "share",
    "man",
    "doc",
    "docs",
    "info",
    "pkgconfig",
    "cmake",
    "__pycache__",
];

/// Compare remaining review dependents with the patched library's exports.
///
/// A defined dynamic symbol of the dependent that is also an export of a
/// patched module is a hit: that row becomes must-rebuild. No hit, and a
/// readable export set, becomes no-rebuild. An empty export set stays in
/// review, because the scan had nothing to look for.
pub fn apply_symbol_check(rows: &mut [RebuildRow], prefix: &Path, notes: &mut BTreeSet<String>) {
    let software = prefix.join("software");
    let mut modules_for: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows.iter() {
        if row.edge_kind == "changed" {
            modules_for
                .entry(row.changed.clone())
                .or_default()
                .push(row.dependent.clone());
        }
    }
    let mut exports_for: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut defined_for: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows.iter_mut() {
        if row.class != CLASS_REVIEW {
            continue;
        }
        let dependent_dir = software.join(&row.dependent);
        if !dependent_dir.is_dir() {
            notes.insert(format!("{}: install directory not found", row.dependent));
            continue;
        }
        let defined = defined_for
            .entry(row.dependent.clone())
            .or_insert_with(|| defined_dynamic_symbols(&dependent_dir));
        let files: Vec<String> = row.changed.split(", ").map(str::to_string).collect();
        let mut hits: Vec<(String, Vec<String>)> = Vec::new();
        let mut unread: Vec<String> = Vec::new();
        for file in &files {
            let exports = exports_for.entry(file.clone()).or_insert_with(|| {
                let mut symbols = BTreeSet::new();
                for module in modules_for.get(file).into_iter().flatten() {
                    let dir = software.join(module);
                    if dir.is_dir() {
                        symbols.extend(exported_symbols(&dir));
                    }
                }
                symbols
            });
            if exports.is_empty() {
                unread.push(file.clone());
                continue;
            }
            let mut matched: Vec<String> = exports.intersection(defined).cloned().collect();
            matched.sort();
            if !matched.is_empty() {
                hits.push((file.clone(), matched));
            }
        }
        if !hits.is_empty() {
            row.changed = hits
                .iter()
                .map(|(file, _)| file.clone())
                .collect::<Vec<_>>()
                .join(", ");
            row.class = CLASS_MUST_REBUILD.to_string();
            row.edge_kind = "builddependency".to_string();
            row.reason = format_hits(&hits);
        } else if unread.is_empty() {
            let evidence = archive_evidence(&row.reason);
            row.class = CLASS_NO_REBUILD.to_string();
            row.reason = format!("no defined dynamic symbol of the patched library; {evidence}");
        } else {
            row.reason = format!(
                "{}; no exported symbols read for {}",
                row.reason,
                unread.join(", ")
            );
        }
    }
}

fn archive_evidence(reason: &str) -> String {
    if let Some(rest) = reason.strip_prefix("builddependency only of ") {
        return rest
            .split("; it may have linked")
            .next()
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    if let Some(rest) = reason.split("static archives:").nth(1) {
        return format!("static archives: {}", rest.trim());
    }
    reason.to_string()
}

fn format_hits(hits: &[(String, Vec<String>)]) -> String {
    hits.iter()
        .map(|(file, symbols)| {
            let shown: Vec<&str> = symbols.iter().take(SAMPLE).map(String::as_str).collect();
            if symbols.len() > SAMPLE {
                format!(
                    "defined symbols of {file}: {} ({} symbols)",
                    shown.join(", "),
                    symbols.len()
                )
            } else {
                format!("defined symbols of {file}: {}", shown.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn exported_symbols(module_dir: &Path) -> BTreeSet<String> {
    let mut symbols = BTreeSet::new();
    for lib in lib_dirs(module_dir) {
        let Ok(entries) = std::fs::read_dir(&lib) else {
            continue;
        };
        let mut files: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        files.sort_by_key(|entry| entry.file_name());
        for entry in files {
            let path = entry.path();
            if path.is_symlink() || !path.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains(".so") {
                symbols.extend(dynamic_defined(&path));
            }
        }
    }
    if !symbols.is_empty() {
        return symbols;
    }
    for lib in lib_dirs(module_dir) {
        let Ok(entries) = std::fs::read_dir(&lib) else {
            continue;
        };
        let mut files: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        files.sort_by_key(|entry| entry.file_name());
        for entry in files {
            let path = entry.path();
            if path.is_symlink() || !path.is_file() {
                continue;
            }
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".a") {
                symbols.extend(archive_defined(&path));
            }
        }
    }
    symbols
}

fn lib_dirs(module_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(module_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    dirs.sort_by_key(|entry| entry.file_name());
    dirs.into_iter()
        .filter(|entry| {
            let path = entry.path();
            if path.is_symlink() {
                return false;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("lib") && path.is_dir()
        })
        .map(|entry| entry.path())
        .collect()
}

fn defined_dynamic_symbols(dir: &Path) -> BTreeSet<String> {
    let mut symbols = BTreeSet::new();
    walk_files(dir, &mut |path| {
        symbols.extend(dynamic_defined(path));
    });
    symbols
}

fn walk_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk_files(&path, visit);
        } else if path.is_file() {
            visit(&path);
        }
    }
}

fn dynamic_defined(path: &Path) -> BTreeSet<String> {
    let Some(bytes) = read_elf(path) else {
        return BTreeSet::new();
    };
    let Ok(file) = object::File::parse(&*bytes) else {
        return BTreeSet::new();
    };
    defined_names(file.dynamic_symbols(), false)
}

fn read_elf(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"\x7fELF" {
        return None;
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&magic);
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn archive_defined(path: &Path) -> BTreeSet<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeSet::new();
    };
    let Ok(archive) = object::read::archive::ArchiveFile::parse(&*bytes) else {
        return BTreeSet::new();
    };
    let mut symbols = BTreeSet::new();
    for member in archive.members() {
        let Ok(member) = member else {
            continue;
        };
        let Ok(data) = member.data(&*bytes) else {
            continue;
        };
        let Ok(file) = object::File::parse(data) else {
            continue;
        };
        symbols.extend(defined_names(file.symbols(), true));
    }
    symbols
}

fn defined_names<'a, I, S>(symbols: I, global_only: bool) -> BTreeSet<String>
where
    I: Iterator<Item = S>,
    S: ObjectSymbol<'a>,
{
    let mut names = BTreeSet::new();
    for symbol in symbols {
        if symbol.is_undefined() {
            continue;
        }
        if global_only && !(symbol.is_global() || symbol.is_weak()) {
            continue;
        }
        if matches!(
            symbol.kind(),
            SymbolKind::File | SymbolKind::Section | SymbolKind::Unknown
        ) {
            continue;
        }
        let Ok(name) = symbol.name() else {
            continue;
        };
        let name = plain_symbol(name);
        if name.is_empty() || name.starts_with('$') {
            continue;
        }
        names.insert(name.to_string());
    }
    names
}

fn plain_symbol(name: &str) -> &str {
    let name = name.split_once("@@").map(|(bare, _)| bare).unwrap_or(name);
    name.split_once('@').map(|(bare, _)| bare).unwrap_or(name)
}

fn reason_with_archives(reason: &str, names: &[String]) -> String {
    let head = reason
        .split("; it may have linked")
        .next()
        .unwrap_or(reason)
        .trim()
        .trim_end_matches(';')
        .trim();
    format!("{head}; static archives: {}", names.join(", "))
}
