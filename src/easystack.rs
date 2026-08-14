//! Emit an EasyBuild easystack file from a solved lock.
//!
//! A lock says what to install and in which order it was selected. An easystack
//! is the format EasyBuild itself consumes for that, `eb --easystack file.yml`,
//! and the format EESSI's software layer is built from, where the files live at
//! `easystacks/software.eessi.io/<version>/` and are the unit of a build PR.
//! Emitting one turns a solve into something both a site pipeline and an
//! upstream contribution can run without a human retyping the list.
//!
//! The format, since EasyBuild 4.7: a top-level `easyconfigs` key whose entries
//! are either a filename or a single-key mapping from filename to `options`,
//! where each option is an `eb` command-line flag without its dashes.

use crate::domain::StackLock;
use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;

/// Per-easyconfig `eb` options, keyed by the easyconfig filename they apply to.
///
/// Keys are written as they appear on the command line without the leading
/// dashes, which is what EasyBuild reads: `from-commit`, `accept-eula-for`,
/// `include-systems`.
pub type EasystackOptions = BTreeMap<String, BTreeMap<String, String>>;

/// The easyconfig filename an entry names, from the path the lock recorded.
///
/// A lock carries the path a candidate was parsed from, which may be absolute,
/// relative to a robot root, or empty for a candidate built in memory. An
/// easystack names files, and EasyBuild finds them through the robot path, so
/// only the basename belongs here.
fn easyconfig_filename(easyconfig_path: &str) -> Option<String> {
    let name = easyconfig_path.rsplit('/').next()?.trim();
    if name.is_empty() || !name.ends_with(".eb") {
        return None;
    }
    Some(name.to_string())
}

/// Render a lock as an easystack document.
///
/// Entries keep the order of the lock rather than being re-sorted, so a diff
/// between two easystacks reads the same way as a diff between the locks they
/// came from. A package whose lock entry names no easyconfig file is skipped:
/// EasyBuild cannot be asked to build a file that was never on disk, and a
/// silently wrong filename would fail deep in a pipeline rather than here.
pub fn lock_to_easystack(lock: &StackLock, options: &EasystackOptions) -> String {
    let paths: Vec<&str> = lock
        .packages
        .iter()
        .map(|p| p.easyconfig_path.as_str())
        .collect();
    easystack_from_paths(&paths, options)
}

/// Render an easystack from easyconfig paths in the order they are given.
///
/// A lock is sorted by name so two locks diff cleanly, which is the wrong
/// order for a build. A sequence from [`crate::build_order`] is already the
/// order to build in, and writing it out sorted would throw away the only
/// thing it knows that a lock does not.
pub fn easystack_from_paths(paths: &[&str], options: &EasystackOptions) -> String {
    let mut entries: Vec<Value> = Vec::new();
    for path in paths {
        let Some(filename) = easyconfig_filename(path) else {
            continue;
        };
        match options.get(&filename) {
            Some(opts) if !opts.is_empty() => {
                let mut option_map = Mapping::new();
                for (key, value) in opts {
                    option_map.insert(Value::String(key.clone()), parse_scalar(value));
                }
                let mut body = Mapping::new();
                body.insert(Value::String("options".into()), Value::Mapping(option_map));
                let mut entry = Mapping::new();
                entry.insert(Value::String(filename), Value::Mapping(body));
                entries.push(Value::Mapping(entry));
            }
            _ => entries.push(Value::String(filename)),
        }
    }

    let mut doc = Mapping::new();
    doc.insert(
        Value::String("easyconfigs".into()),
        Value::Sequence(entries),
    );
    serde_yaml::to_string(&Value::Mapping(doc)).unwrap_or_default()
}

/// Read an option value as the type EasyBuild expects.
///
/// `debug: True` is a boolean to EasyBuild and `from-commit: 4bda830` is a
/// string. YAML would read a bare `4bda830`-style value as a string already,
/// but a numeric-looking one such as a PR number has to stay a number, and a
/// version-shaped value such as `parallel: "1"` has to stay a string, which is
/// why the caller's quoting is preserved rather than re-guessed.
fn parse_scalar(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        return Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    match trimmed {
        "True" | "true" => Value::Bool(true),
        "False" | "false" => Value::Bool(false),
        _ => match trimmed.parse::<i64>() {
            Ok(n) => Value::Number(n.into()),
            Err(_) => Value::String(trimmed.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LockPackage, SolverMeta, Toolchain};

    fn toolchain() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        }
    }

    fn lock_with(paths: &[&str]) -> StackLock {
        StackLock {
            schema_version: 1,
            toolchain: toolchain(),
            generation_label: Some("2025a".into()),
            packages: paths
                .iter()
                .enumerate()
                .map(|(i, path)| LockPackage {
                    name: format!("Pkg{i}"),
                    version: "1.0".into(),
                    toolchain: toolchain(),
                    versionsuffix: None,
                    easyconfig_path: (*path).to_string(),
                })
                .collect(),
            solver: SolverMeta {
                engine: "resolvo".into(),
                engine_version: "0.0.0".into(),
                timestamp: "2026-08-12T00:00:00Z".into(),
            },
        }
    }

    #[test]
    fn a_plain_lock_becomes_a_list_of_filenames() {
        let lock = lock_with(&[
            "/robot/a/Alpha/Alpha-1.0-foss-2025a.eb",
            "b/Beta/Beta-2.0-foss-2025a.eb",
        ]);
        let yaml = lock_to_easystack(&lock, &EasystackOptions::new());
        assert_eq!(
            yaml,
            "easyconfigs:\n- Alpha-1.0-foss-2025a.eb\n- Beta-2.0-foss-2025a.eb\n"
        );
    }

    #[test]
    fn options_turn_an_entry_into_the_mapping_form() {
        let lock = lock_with(&["c/CUDA/CUDA-12.8.0.eb"]);
        let mut options = EasystackOptions::new();
        options.insert(
            "CUDA-12.8.0.eb".into(),
            BTreeMap::from([("accept-eula-for".to_string(), "CUDA".to_string())]),
        );
        let yaml = lock_to_easystack(&lock, &options);
        assert!(yaml.contains("- CUDA-12.8.0.eb:"), "{yaml}");
        assert!(yaml.contains("    accept-eula-for: CUDA"), "{yaml}");
        assert!(yaml.contains("  options:"), "{yaml}");
    }

    #[test]
    fn the_document_parses_back_as_easybuild_would_read_it() {
        let lock = lock_with(&["c/CUDA/CUDA-12.8.0.eb", "g/GCC/GCC-14.2.0.eb"]);
        let mut options = EasystackOptions::new();
        options.insert(
            "CUDA-12.8.0.eb".into(),
            BTreeMap::from([
                ("accept-eula-for".to_string(), "CUDA".to_string()),
                ("debug".to_string(), "True".to_string()),
                ("parallel".to_string(), "\"1\"".to_string()),
            ]),
        );
        let yaml = lock_to_easystack(&lock, &options);
        let parsed: Value = serde_yaml::from_str(&yaml).expect("valid YAML");
        let list = parsed["easyconfigs"].as_sequence().unwrap();
        assert_eq!(list.len(), 2);
        let cuda = list[0].as_mapping().unwrap();
        let body = &cuda[&Value::String("CUDA-12.8.0.eb".into())]["options"];
        assert_eq!(body["accept-eula-for"], Value::String("CUDA".into()));
        assert_eq!(body["debug"], Value::Bool(true));
        // A quoted value stays a string: EasyBuild reads parallel as a string.
        assert_eq!(body["parallel"], Value::String("1".into()));
        assert_eq!(list[1], Value::String("GCC-14.2.0.eb".into()));
    }

    /// A candidate parsed from memory has no file to name, and inventing one
    /// would fail in the pipeline instead of here.
    #[test]
    fn a_package_with_no_easyconfig_file_is_left_out() {
        let lock = lock_with(&["", "x/X/X-1.0.eb", "not-an-easyconfig.txt"]);
        let yaml = lock_to_easystack(&lock, &EasystackOptions::new());
        assert_eq!(yaml, "easyconfigs:\n- X-1.0.eb\n");
    }
}
