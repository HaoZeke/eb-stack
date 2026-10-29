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
/// Read one easystack option as text. A YAML number (a pull request id)
/// is rendered in decimal; anything else that is not a string is ignored.
fn option_string(options: Option<&Value>, key: &str) -> Option<String> {
    let value = options?.get(key)?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    None
}

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

/// One entry of an easystack as written: the easyconfig file it names, and the
/// commit that file comes from when the entry pins one with `from-commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EasystackEntry {
    /// Easyconfig filename. A path in the file (EESSI's per-arch stacks write
    /// absolute reprod paths) is cut to its basename, which is what EasyBuild
    /// looks up on the robot path.
    pub file: String,
    /// The easybuild-easyconfigs commit given as `from-commit`, if any.
    pub from_commit: Option<String>,
    /// The easybuild-easyblocks commit given as `include-easyblocks-from-commit`.
    pub include_easyblocks_from_commit: Option<String>,
    /// The easybuild-easyblocks pull request given as `include-easyblocks-from-pr`.
    pub include_easyblocks_from_pr: Option<String>,
}

/// Read the entries of an easystack document, in file order.
///
/// Both entry shapes are read: a bare filename, and a single-key mapping from
/// filename to `options`. Anything under the top-level `easyconfigs` key that
/// names no `.eb` file is skipped rather than guessed at.
pub fn parse_easystack_entries(text: &str) -> Result<Vec<EasystackEntry>, serde_yaml::Error> {
    let doc: Value = serde_yaml::from_str(text)?;
    let Some(items) = doc.get("easyconfigs").and_then(Value::as_sequence) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in items {
        let (name, options) = match item {
            Value::String(name) => (name.as_str(), None),
            Value::Mapping(map) if map.len() == 1 => {
                let (key, value) = map.iter().next().expect("one key");
                let Some(name) = key.as_str() else { continue };
                (name, value.get("options"))
            }
            _ => continue,
        };
        let Some(file) = easyconfig_filename(name) else {
            continue;
        };
        let from_commit = option_string(options, "from-commit");
        let include_easyblocks_from_commit =
            option_string(options, "include-easyblocks-from-commit");
        let include_easyblocks_from_pr = option_string(options, "include-easyblocks-from-pr");
        out.push(EasystackEntry {
            file,
            from_commit,
            include_easyblocks_from_commit,
            include_easyblocks_from_pr,
        });
    }
    Ok(out)
}

/// Where entries go in EESSI's software layer.
///
/// `easystacks/software.eessi.io/<V>/eessi-<V>-eb-<X>-<generation>.yml` for the
/// generation's toolchains, `...-001-system.yml` for toolchain-free recipes,
/// and the same names under `accel/nvidia/` for CUDA builds. One file per
/// EasyBuild release and generation is how the existing EESSI versions are laid
/// out, and new entries go into the newest release's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EessiLayout {
    /// EESSI version, `2026.06`.
    pub eessi_version: String,
    /// EasyBuild release the entries are built with, `5.4.1`.
    pub easybuild_version: String,
    /// Toolchain generation, `2026.1`.
    pub generation: String,
    /// Toolchain labels of the generation: `GCCcore-15.2.0`, `foss-2026.1`, ...
    pub members: Vec<String>,
}

/// Toolchain names whose presence in a filename means it is not a system recipe.
const TOOLCHAIN_NAME: &str = r"-(GCCcore|GCC|foss|gompi|gfbf|intel|iimpi|iimkl|intel-compilers|lfoss|lompi|lfbf|llvm-compilers|nvompi|nvofbf|NVHPC|nvidia-compilers|iccifort|fosscuda|gcccuda)-[0-9]";

impl EessiLayout {
    /// The easystack path, relative to the software-layer root, for one easyconfig.
    ///
    /// Errors when the file names a toolchain outside this generation: placing
    /// it would put a 2025b recipe into a 2026.1 file.
    pub fn path_for(&self, file: &str) -> Result<String, String> {
        let base = format!("easystacks/software.eessi.io/{}", self.eessi_version);
        let sub = if file.contains("-CUDA-") {
            "accel/nvidia/"
        } else {
            ""
        };
        let in_generation = self
            .members
            .iter()
            .filter(|m| m.as_str() != "system")
            .any(|m| file.contains(&format!("-{m}.eb")) || file.contains(&format!("-{m}-")));
        let tail = if in_generation {
            self.generation.clone()
        } else if regex::Regex::new(TOOLCHAIN_NAME)
            .expect("static pattern")
            .is_match(file)
        {
            return Err(format!(
                "{file} uses a toolchain outside generation {} ({})",
                self.generation,
                self.members.join(", ")
            ));
        } else {
            "001-system".to_string()
        };
        Ok(format!(
            "{base}/{sub}eessi-{v}-eb-{x}-{tail}.yml",
            v = self.eessi_version,
            x = self.easybuild_version
        ))
    }
}

/// Split easyconfig paths into EESSI easystack files, keeping the given order
/// inside each file. Keys are paths relative to the software-layer root.
pub fn eessi_easystacks(
    paths: &[&str],
    options: &EasystackOptions,
    layout: &EessiLayout,
) -> Result<BTreeMap<String, String>, String> {
    let mut grouped: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for path in paths {
        let Some(file) = easyconfig_filename(path) else {
            continue;
        };
        grouped
            .entry(layout.path_for(&file)?)
            .or_default()
            .push(path);
    }
    Ok(grouped
        .into_iter()
        .map(|(target, files)| (target, easystack_from_paths(&files, options)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> EessiLayout {
        EessiLayout {
            eessi_version: "2026.06".into(),
            easybuild_version: "5.4.1".into(),
            generation: "2026.1".into(),
            members: [
                "system",
                "GCCcore-15.2.0",
                "GCC-15.2.0",
                "gfbf-2026.1",
                "gompi-2026.1",
                "foss-2026.1",
            ]
            .map(String::from)
            .to_vec(),
        }
    }

    #[test]
    fn eessi_layout_places_generation_system_and_cuda_entries() {
        let l = layout();
        let root = "easystacks/software.eessi.io/2026.06";
        assert_eq!(
            l.path_for("BLAST+-2.17.0-gompi-2026.1.eb").unwrap(),
            format!("{root}/eessi-2026.06-eb-5.4.1-2026.1.yml")
        );
        assert_eq!(
            l.path_for("Julia-1.13.2.eb").unwrap(),
            format!("{root}/eessi-2026.06-eb-5.4.1-001-system.yml")
        );
        assert_eq!(
            l.path_for("Maven-3.9.11-Java-21.eb").unwrap(),
            format!("{root}/eessi-2026.06-eb-5.4.1-001-system.yml")
        );
        assert_eq!(
            l.path_for("NCCL-2.30.4-GCCcore-15.2.0-CUDA-13.3.0.eb")
                .unwrap(),
            format!("{root}/accel/nvidia/eessi-2026.06-eb-5.4.1-2026.1.yml")
        );
        assert_eq!(
            l.path_for("FastQC-0.13.0-GCCcore-15.2.0-Java-25.eb")
                .unwrap(),
            format!("{root}/eessi-2026.06-eb-5.4.1-2026.1.yml")
        );
        let err = l.path_for("Bowtie2-2.5.4-GCC-14.2.0.eb").unwrap_err();
        assert!(err.contains("outside generation 2026.1"), "{err}");
    }

    #[test]
    fn eessi_easystacks_split_by_file_and_keep_options() {
        let mut options = EasystackOptions::new();
        options.entry("Julia-1.13.2.eb".into()).or_default().insert(
            "from-commit".into(),
            "17c84d79a1fe815c82860acce20ba262d7dd2f49".into(),
        );
        let files = eessi_easystacks(
            &[
                "a/HMMER-3.4-gompi-2026.1.eb",
                "j/Julia-1.13.2.eb",
                "t/tmux-3.6b-GCCcore-15.2.0.eb",
            ],
            &options,
            &layout(),
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        let generation =
            &files["easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.1-2026.1.yml"];
        assert_eq!(
            generation,
            "easyconfigs:\n- HMMER-3.4-gompi-2026.1.eb\n- tmux-3.6b-GCCcore-15.2.0.eb\n"
        );
        let system =
            &files["easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.1-001-system.yml"];
        assert!(system.contains("Julia-1.13.2.eb"), "{system}");
        assert!(
            system.contains("from-commit: 17c84d79a1fe815c82860acce20ba262d7dd2f49"),
            "{system}"
        );
    }
}

#[cfg(test)]
mod lock_tests {
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
                criteria: Vec::new(),
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

    #[test]
    fn entries_are_read_in_both_shapes_with_their_commit() {
        let text = "easyconfigs:\n  - GROMACS-2026.0-foss-2026.1.eb\n  - /cvmfs/x/reprod/ELPA-2025.06.002-foss-2026.1.eb\n  - Valgrind-3.26.0-gompi-2026.1.eb:\n      options:\n        from-commit: 0123456789abcdef0123456789abcdef01234567\n";
        let got = parse_easystack_entries(text).unwrap();
        assert_eq!(
            got,
            vec![
                EasystackEntry {
                    file: "GROMACS-2026.0-foss-2026.1.eb".into(),
                    from_commit: None,
                    include_easyblocks_from_commit: None,
                    include_easyblocks_from_pr: None,
                },
                EasystackEntry {
                    file: "ELPA-2025.06.002-foss-2026.1.eb".into(),
                    from_commit: None,
                    include_easyblocks_from_commit: None,
                    include_easyblocks_from_pr: None,
                },
                EasystackEntry {
                    file: "Valgrind-3.26.0-gompi-2026.1.eb".into(),
                    from_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
                    include_easyblocks_from_commit: None,
                    include_easyblocks_from_pr: None,
                },
            ]
        );
    }
}
