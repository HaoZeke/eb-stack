//! Facts read from an installed EESSI prefix.
//!
//! The install directory of a module `Name/version` is
//! `<prefix>/software/Name/version`. A static archive is a `*.a` file
//! directly under a `lib*` directory there (`lib/libz.a`, `lib64/libz.a`).

use crate::rebuild_set::{RebuildError, RebuildRow, CLASS_NO_REBUILD, CLASS_REVIEW};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
