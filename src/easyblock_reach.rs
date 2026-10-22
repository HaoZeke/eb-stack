//! Which easyblock classes a changed Python file reaches.
//!
//! The class hierarchy is the `class` statement, not an import of the base:
//! `EB_GROMACS(CMakeMake)` is reached by a change to `CMakeMake`. A framework
//! module reaches the easyblocks that import it, and then their subclasses.
//! Easyconfig matching is layered on top of this index.

use crate::build_order::{module_name, ModuleKey};
use crate::eb_easyblock::encode_class_name;
use crate::eb_parse::easyblock_sites;
use rustpython_parser::ast::{self, Stmt};
use rustpython_parser::Parse;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Schema of [`ReachReport`].
pub const REACH_SCHEMA_VERSION: u32 = 1;

/// What a class declares about itself, read from its source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassFact {
    /// Base classes as written, last component or dotted.
    pub bases: Vec<String>,
    /// `self.cfg['exts_defaultclass'] = '...'` in the class body.
    pub exts_default: Option<String>,
    /// `default_easyblock` assigned in `extra_options`.
    pub component_default: Option<String>,
}

/// Easyblock classes and the modules that import them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EasyblockIndex {
    /// Class name to what its source declares.
    pub classes: BTreeMap<String, ClassFact>,
    /// Python module to the classes defined in files that import it.
    pub importers: BTreeMap<String, BTreeSet<String>>,
}

/// Index every `.py` file under `roots`. Earlier roots win a class name.
pub fn index_roots(roots: &[PathBuf]) -> Result<EasyblockIndex, String> {
    let mut files = Vec::new();
    for root in roots {
        collect_py(root, &mut files)
            .map_err(|error| format!("read {}: {error}", root.display()))?;
    }
    let mut index = EasyblockIndex::default();
    for path in files {
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let module = python_module(&path);
        let (classes, imports) =
            read_python(&source, &path.display().to_string(), module.as_deref())
                .map_err(|error| format!("parse {}: {error}", path.display()))?;
        let names: Vec<String> = classes.iter().map(|(name, _)| name.clone()).collect();
        for (name, fact) in classes {
            index.classes.entry(name).or_insert(fact);
        }
        if let Some(module) = module.as_deref() {
            for imported in imports {
                if imported != module {
                    index
                        .importers
                        .entry(imported)
                        .or_default()
                        .extend(names.iter().cloned());
                }
            }
        }
    }
    Ok(index)
}

/// Proper subclasses of `class_name`, including subclasses of subclasses.
pub fn descendants(index: &EasyblockIndex, class_name: &str) -> BTreeSet<String> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, fact) in &index.classes {
        for base in &fact.bases {
            children
                .entry(base.as_str())
                .or_default()
                .push(name.as_str());
            if let Some(short) = base.rsplit('.').next() {
                if short != base {
                    children.entry(short).or_default().push(name.as_str());
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    let mut stack = vec![class_name];
    let mut seen = HashSet::new();
    while let Some(name) = stack.pop() {
        if !seen.insert(name.to_string()) {
            continue;
        }
        if let Some(kids) = children.get(name) {
            for kid in kids {
                if *kid != class_name {
                    out.insert((*kid).to_string());
                    stack.push(kid);
                }
            }
        }
    }
    out
}

/// Classes named by a change, and those plus every subclass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassReach {
    /// Defined in the changed file, or defined in a file that imports it.
    pub changed: BTreeSet<String>,
    /// `changed` plus subclasses.
    pub reached: BTreeSet<String>,
}

/// Classes a changed file defines, classes that import its module, and subclasses.
pub fn affected_classes(
    index: &EasyblockIndex,
    path: &Path,
    source: &str,
) -> Result<ClassReach, String> {
    let module = python_module(path);
    let (classes, _) = read_python(source, &path.display().to_string(), module.as_deref())?;
    let mut changed: BTreeSet<String> = classes.into_iter().map(|(name, _)| name).collect();
    if let Some(module) = module {
        if let Some(users) = index.importers.get(&module) {
            changed.extend(users.iter().cloned());
        }
    }
    let mut reached = changed.clone();
    for name in &changed {
        reached.extend(descendants(index, name));
    }
    Ok(ClassReach { changed, reached })
}

/// `easybuild.tools.filetools` for `.../easybuild/tools/filetools.py`.
pub fn python_module(path: &Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let at = text.find("easybuild/")?;
    let rest = text[at..].strip_suffix(".py")?;
    let rest = rest.strip_suffix("/__init__").unwrap_or(rest);
    Some(rest.replace('/', "."))
}

type ParsedPython = (Vec<(String, ClassFact)>, BTreeSet<String>);

fn read_python(source: &str, path: &str, module: Option<&str>) -> Result<ParsedPython, String> {
    let suite = ast::Suite::parse(source, path).map_err(|error| error.to_string())?;
    let mut imports = BTreeSet::new();
    collect_imports(&suite, module.unwrap_or(""), &mut imports);
    let classes = suite
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::ClassDef(class) => Some(class),
            _ => None,
        })
        .map(|class| {
            (
                class.name.to_string(),
                ClassFact {
                    bases: class.bases.iter().filter_map(dotted_name).collect(),
                    exts_default: assigned_string(&class.body, "exts_defaultclass"),
                    component_default: assigned_string(&class.body, "default_easyblock"),
                },
            )
        })
        .collect();
    Ok((classes, imports))
}

fn dotted_name(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            dotted_name(&attr.value).map(|base| format!("{base}.{}", attr.attr))
        }
        _ => None,
    }
}

fn assigned_string(stmts: &[Stmt], key: &str) -> Option<String> {
    let mut found = None;
    for stmt in stmts {
        let body: &[Stmt] = match stmt {
            Stmt::FunctionDef(function) => &function.body,
            _ => continue,
        };
        for inner in body {
            let Stmt::Assign(assign) = inner else {
                continue;
            };
            let mut names = Vec::new();
            for target in &assign.targets {
                subscript_keys(target, &mut names);
            }
            if names.iter().any(|name| name == key) {
                if let Some(value) = string_constant(&assign.value) {
                    found = Some(value);
                }
            }
        }
    }
    found
}

fn subscript_keys(expr: &ast::Expr, keys: &mut Vec<String>) {
    if let ast::Expr::Subscript(subscript) = expr {
        if let Some(key) = string_constant(&subscript.slice) {
            keys.push(key);
        }
        subscript_keys(&subscript.value, keys);
    }
}

fn string_constant(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Str(text) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn collect_imports(stmts: &[Stmt], module: &str, imports: &mut BTreeSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    imports.insert(alias.name.to_string());
                }
            }
            Stmt::ImportFrom(from) => record_from(from, module, imports),
            Stmt::FunctionDef(function) => collect_imports(&function.body, module, imports),
            Stmt::ClassDef(class) => collect_imports(&class.body, module, imports),
            Stmt::If(branch) => {
                collect_imports(&branch.body, module, imports);
                collect_imports(&branch.orelse, module, imports);
            }
            _ => {}
        }
    }
}

fn record_from(from: &ast::StmtImportFrom, module: &str, imports: &mut BTreeSet<String>) {
    let level = from
        .level
        .as_ref()
        .map(|level| level.to_usize())
        .unwrap_or(0);
    let base = if level == 0 {
        String::new()
    } else {
        relative_base(module, level)
    };
    let named = from.module.as_ref().map(|name| name.as_str()).unwrap_or("");
    let joined = match (base.is_empty(), named.is_empty()) {
        (true, _) => named.to_string(),
        (false, true) => base,
        (false, false) => format!("{base}.{named}"),
    };
    if !joined.is_empty() {
        imports.insert(joined.clone());
    }
    for alias in &from.names {
        let name = alias.name.as_str();
        if name != "*" && !joined.is_empty() {
            imports.insert(format!("{joined}.{name}"));
        }
    }
}

fn relative_base(module: &str, level: usize) -> String {
    let mut parts: Vec<&str> = module
        .rsplit_once('.')
        .map(|(package, _)| package)
        .unwrap_or("")
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    for _ in 1..level {
        parts.pop();
    }
    parts.join(".")
}

fn collect_py(root: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
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
            collect_py(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "py") {
            files.push(path);
        }
    }
    Ok(())
}

/// Header on every generated TOML file.
pub const GENERATED_HEADER: &str = "# Generated by `eb-stack easyblock-reach`. Data only: the branch holding this file is\n# regenerated on every run and must never be merged.\n";

/// Why a reach report could not be built.
#[derive(Debug, Error)]
pub enum ReachError {
    /// A parse or argument failure, already worded for the CLI.
    #[error("{0}")]
    Message(String),
    /// Reading an easyconfig or easyblock tree failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What to resolve. Extension and reverse rows are filled by a later pass.
#[derive(Debug, Clone)]
pub struct ReachQuery {
    /// Easyblock and framework roots. Earlier roots win a class name.
    pub roots: Vec<PathBuf>,
    /// Easyconfig files or directories.
    pub easyconfigs: Vec<PathBuf>,
    /// Changed path and the Python source classes are read from.
    pub changed: Vec<(PathBuf, String)>,
    /// Installed module names (`name/version[-toolchain]versionsuffix`).
    pub installed: BTreeSet<String>,
    /// Timestamp stored on the report.
    pub generated_at: String,
}

/// One easyconfig whose top-level easyblock is in the reached set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachHit {
    /// Easyconfig path.
    pub easyconfig: String,
    /// Module that easyconfig installs.
    pub module: String,
    /// Easyblock class that matched.
    pub class_name: String,
    /// `easyblock` when the recipe names one, otherwise `implied`.
    pub via: String,
    /// True when `module` is in the installed set.
    pub installed: bool,
}

/// Easyconfigs reached by a change to an easyblock or a framework module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachReport {
    /// [`REACH_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Copied from the query.
    pub generated_at: String,
    /// Changed Python paths.
    pub changed_files: Vec<String>,
    /// Classes defined in those files, plus classes that import them.
    pub changed_classes: Vec<String>,
    /// `changed_classes` plus subclasses.
    pub reached_classes: Vec<String>,
    /// Distinct easyconfigs in `hits`.
    pub easyconfig_count: usize,
    /// Distinct installed modules in `hits`.
    pub installed_count: usize,
    /// One row per reached easyconfig.
    pub hits: Vec<ReachHit>,
    /// Easyconfigs that could not be parsed.
    pub skipped: Vec<String>,
}

/// Which easyconfigs use a reached class, by the `easyblock` line or `EB_<Name>`.
pub fn reach(query: &ReachQuery) -> Result<ReachReport, ReachError> {
    let index = index_roots(&query.roots).map_err(ReachError::Message)?;
    let mut changed = BTreeSet::new();
    let mut reached = BTreeSet::new();
    let mut changed_files = Vec::new();
    for (path, source) in &query.changed {
        let one = affected_classes(&index, path, source).map_err(ReachError::Message)?;
        changed.extend(one.changed);
        reached.extend(one.reached);
        changed_files.push(path.display().to_string());
    }
    changed_files.sort();
    let mut files = Vec::new();
    for root in &query.easyconfigs {
        collect_eb(root, &mut files)?;
    }
    let mut hits = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        let src = std::fs::read_to_string(&path)?;
        let (mut recipe, _) = match easyblock_sites(&src) {
            Ok(pair) => pair,
            Err(error) => {
                skipped.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        recipe.easyconfig_path = path.display().to_string();
        let named = recipe.easyblock.is_some();
        let class_name = recipe
            .easyblock
            .clone()
            .unwrap_or_else(|| encode_class_name(&recipe.name));
        if !reached.contains(&class_name) {
            continue;
        }
        let key = ModuleKey::of(&recipe.to_candidate());
        let module = module_name(&key);
        hits.push(ReachHit {
            easyconfig: recipe.easyconfig_path,
            module: module.clone(),
            class_name,
            via: if named { "easyblock" } else { "implied" }.to_string(),
            installed: query.installed.contains(&module),
        });
    }
    skipped.sort();
    hits.sort_by(|left, right| left.easyconfig.cmp(&right.easyconfig));
    let easyconfig_count = hits
        .iter()
        .map(|hit| hit.easyconfig.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let installed_count = hits
        .iter()
        .filter(|hit| hit.installed)
        .map(|hit| hit.module.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    Ok(ReachReport {
        schema_version: REACH_SCHEMA_VERSION,
        generated_at: query.generated_at.clone(),
        changed_files,
        changed_classes: changed.into_iter().collect(),
        reached_classes: reached.into_iter().collect(),
        easyconfig_count,
        installed_count,
        hits,
        skipped,
    })
}

fn collect_eb(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), ReachError> {
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
            collect_eb(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "eb") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn recipe(name: &str, version: &str, easyblock: &str) -> String {
        let line = if easyblock.is_empty() {
            String::new()
        } else {
            format!("easyblock = '{easyblock}'\n")
        };
        format!("name = '{name}'\nversion = '{version}'\ntoolchain = SYSTEM\n{line}")
    }

    #[test]
    fn subclass_implied_name_and_import_reach_recipes() {
        let root = tempfile::tempdir().unwrap();
        let py = root.path().join("py");
        let ecs = root.path().join("eb");
        write(
            &py,
            "easybuild/easyblocks/generic/configuremake.py",
            "class ConfigureMake:\n    pass\n",
        );
        write(
            &py,
            "easybuild/easyblocks/generic/cmakemake.py",
            "from easybuild.tools.filetools import read_file\nclass CMakeMake(ConfigureMake):\n    pass\n",
        );
        write(
            &py,
            "easybuild/easyblocks/g/gromacs.py",
            "class EB_GROMACS(CMakeMake):\n    pass\n",
        );
        write(
            &py,
            "easybuild/tools/filetools.py",
            "def read_file():\n    return None\n",
        );
        write(&ecs, "gromacs.eb", &recipe("GROMACS", "2024.1", ""));
        write(&ecs, "zlib.eb", &recipe("zlib", "1.3", "CMakeMake"));
        write(&ecs, "make.eb", &recipe("make", "1", "ConfigureMake"));
        let cmake = py.join("easybuild/easyblocks/generic/cmakemake.py");
        let report = reach(&ReachQuery {
            roots: vec![py.clone()],
            easyconfigs: vec![ecs.clone()],
            changed: vec![(cmake.clone(), std::fs::read_to_string(&cmake).unwrap())],
            installed: BTreeSet::from(["zlib/1.3".into()]),
            generated_at: "2026-10-22T00:00:00Z".into(),
        })
        .unwrap();
        assert!(report
            .reached_classes
            .iter()
            .any(|name| name == "EB_GROMACS"));
        let gromacs = report
            .hits
            .iter()
            .find(|hit| hit.easyconfig.ends_with("gromacs.eb"))
            .unwrap();
        assert_eq!(gromacs.via, "implied");
        assert_eq!(gromacs.class_name, "EB_GROMACS");
        assert!(!gromacs.installed);
        let zlib = report
            .hits
            .iter()
            .find(|hit| hit.easyconfig.ends_with("zlib.eb"))
            .unwrap();
        assert_eq!(zlib.via, "easyblock");
        assert!(zlib.installed);
        assert!(report
            .hits
            .iter()
            .all(|hit| !hit.easyconfig.ends_with("make.eb")));
        assert_eq!(report.easyconfig_count, 2);
        assert_eq!(report.installed_count, 1);
        let tools = py.join("easybuild/tools/filetools.py");
        let via_import = reach(&ReachQuery {
            roots: vec![py],
            easyconfigs: vec![ecs],
            changed: vec![(tools.clone(), std::fs::read_to_string(&tools).unwrap())],
            installed: BTreeSet::new(),
            generated_at: "t".into(),
        })
        .unwrap();
        assert!(via_import
            .changed_classes
            .iter()
            .any(|name| name == "CMakeMake"));
        assert!(via_import
            .hits
            .iter()
            .any(|hit| hit.easyconfig.ends_with("gromacs.eb")));
        assert!(via_import
            .hits
            .iter()
            .all(|hit| !hit.easyconfig.ends_with("make.eb")));
    }
}
