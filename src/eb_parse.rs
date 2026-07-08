//! Parse EasyBuild easyconfig (`.eb`) files into structured candidates.
//!
//! Easyconfigs are a restricted Python-like DSL. We do **not** eval Python.
//! Extracted fields: `name`, `version`, `versionsuffix`, `toolchain`,
//! `dependencies`, `builddependencies` (and optional list helpers).

use crate::domain::{Candidate, DepReq, LockPackage, SolverMeta, StackLock, Toolchain};
use crate::version::matches_req;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("IO {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("parse {0}: {1}")]
    Parse(String, String),
}

/// Strip full-line and trailing `#` comments outside quotes (best-effort).
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let mut in_s = false;
        let mut in_d = false;
        let mut cut = line.len();
        let b = line.as_bytes();
        let mut i = 0usize;
        while i < b.len() {
            let c = b[i] as char;
            if c == '\'' && !in_d {
                in_s = !in_s;
            } else if c == '"' && !in_s {
                in_d = !in_d;
            } else if c == '#' && !in_s && !in_d {
                cut = i;
                break;
            }
            i += 1;
        }
        let piece = line[..cut].trim_end();
        if !piece.is_empty() {
            out.push_str(piece);
            out.push('\n');
        }
    }
    out
}

fn assign_string(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix(key)
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('='))
            .map(str::trim_start)
        else {
            continue;
        };
        let bytes = rest.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let q = bytes[0];
        if q == b'\'' || q == b'"' {
            if let Some(end) = rest[1..].find(q as char) {
                return Some(rest[1..1 + end].to_string());
            }
        }
    }
    None
}

fn assign_toolchain(src: &str) -> Option<Toolchain> {
    let mut buf = String::new();
    let mut capturing = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("toolchain") && t.contains('=') {
            capturing = true;
            buf.push_str(t);
            buf.push(' ');
            if t.contains('}') {
                break;
            }
            continue;
        }
        if capturing {
            buf.push_str(t);
            buf.push(' ');
            if t.contains('}') {
                break;
            }
        }
    }
    if buf.is_empty() {
        return None;
    }
    let name_re = regex::Regex::new(r#"['"]name['"]\s*:\s*['"]([^'"]+)['"]"#).ok()?;
    let ver_re = regex::Regex::new(r#"['"]version['"]\s*:\s*['"]([^'"]+)['"]"#).ok()?;
    let name = name_re.captures(&buf)?.get(1)?.as_str().to_string();
    let version = ver_re.captures(&buf)?.get(1)?.as_str().to_string();
    Some(Toolchain { name, version })
}

fn assign_list_body(src: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?m)^\s*{}\s*=\s*\[", regex::escape(key))).ok()?;
    let m = re.find(src)?;
    let start = m.end();
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(src[start..i].to_string());
            }
        }
        i += 1;
    }
    None
}

/// Map EasyBuild dependency version field to a solver requirement string.
fn version_field_to_req(version: &str) -> String {
    let version = version.trim();
    if version.starts_with("==")
        || version.starts_with(">=")
        || version.starts_with("<=")
        || version.starts_with('>')
        || version.starts_with('<')
        || version.starts_with('!')
    {
        version.to_string()
    } else {
        // EasyBuild default: exact co-version pin.
        format!("=={version}")
    }
}

/// Parse dependency tuples from a list body into DepReq.
pub fn parse_dep_list_body(body: &str) -> Vec<DepReq> {
    let mut deps = Vec::new();
    // Match ('Name', 'ver') or ("Name", "ver") without backreferences (regex crate).
    let re_tuple = regex::Regex::new(
        r#"\(\s*'(?P<n1>[^']+)'\s*,\s*'(?P<v1>[^']+)'(?:\s*,\s*'[^']*')?|\(\s*"(?P<n2>[^"]+)"\s*,\s*"(?P<v2>[^"]+)"(?:\s*,\s*"[^"]*")?"#,
    )
    .expect("dep tuple regex");
    for c in re_tuple.captures_iter(body) {
        let name = c
            .name("n1")
            .or_else(|| c.name("n2"))
            .unwrap()
            .as_str()
            .to_string();
        let version = c
            .name("v1")
            .or_else(|| c.name("v2"))
            .unwrap()
            .as_str()
            .to_string();
        deps.push(DepReq {
            name,
            version_req: version_field_to_req(&version),
        });
    }
    // Filename form: 'OpenMPI-4.1.6-foss-2025b.eb'
    let re_file = regex::Regex::new(
        r#"'([A-Za-z0-9_+.-]+)-([0-9][A-Za-z0-9._+]*)-(?:[A-Za-z0-9_+.-]+)\.eb'|"([A-Za-z0-9_+.-]+)-([0-9][A-Za-z0-9._+]*)-(?:[A-Za-z0-9_+.-]+)\.eb""#,
    )
    .expect("dep file regex");
    for c in re_file.captures_iter(body) {
        let (name, version) = if let Some(n) = c.get(1) {
            (n.as_str().to_string(), c.get(2).unwrap().as_str().to_string())
        } else {
            (
                c.get(3).unwrap().as_str().to_string(),
                c.get(4).unwrap().as_str().to_string(),
            )
        };
        if !deps.iter().any(|d| d.name == name) {
            deps.push(DepReq {
                name,
                version_req: version_field_to_req(&version),
            });
        }
    }
    deps
}

/// Parse one `.eb` file into a Candidate.
pub fn parse_easyconfig_file(path: &Path) -> Result<Candidate, ParseError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ParseError::Io(path.display().to_string(), e))?;
    let src = strip_comments(&raw);
    let name = assign_string(&src, "name").ok_or_else(|| {
        ParseError::Parse(path.display().to_string(), "missing name = ...".into())
    })?;
    let version = assign_string(&src, "version").ok_or_else(|| {
        ParseError::Parse(path.display().to_string(), "missing version = ...".into())
    })?;
    let versionsuffix = assign_string(&src, "versionsuffix");
    let toolchain = assign_toolchain(&src).ok_or_else(|| {
        ParseError::Parse(
            path.display().to_string(),
            "missing toolchain = {'name': ..., 'version': ...}".into(),
        )
    })?;

    let mut dependencies = Vec::new();
    if let Some(body) = assign_list_body(&src, "dependencies") {
        dependencies.extend(parse_dep_list_body(&body));
    }
    let mut builddependencies = Vec::new();
    if let Some(body) = assign_list_body(&src, "builddependencies") {
        builddependencies.extend(parse_dep_list_body(&body));
    }

    Ok(Candidate {
        name,
        version,
        toolchain,
        versionsuffix,
        easyconfig_path: path.display().to_string(),
        dependencies,
        builddependencies,
    })
}

/// Walk a directory tree for `*.eb` and parse all easyconfigs.
pub fn parse_easyconfig_tree(root: &Path) -> Result<Vec<Candidate>, ParseError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd =
            std::fs::read_dir(&dir).map_err(|e| ParseError::Io(dir.display().to_string(), e))?;
        for ent in rd {
            let ent = ent.map_err(|e| ParseError::Io(dir.display().to_string(), e))?;
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("eb") {
                out.push(parse_easyconfig_file(&p)?);
            }
        }
    }
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.toolchain.version.cmp(&b.toolchain.version))
    });
    Ok(out)
}

pub fn filter_toolchain(cands: &[Candidate], tc: &Toolchain) -> Vec<Candidate> {
    cands
        .iter()
        .filter(|c| c.toolchain.name == tc.name && c.toolchain.version == tc.version)
        .cloned()
        .collect()
}

pub fn lock_from_candidates(
    cands: &[Candidate],
    generation_label: Option<String>,
    engine: &str,
) -> StackLock {
    let toolchain = cands
        .first()
        .map(|c| c.toolchain.clone())
        .unwrap_or(Toolchain {
            name: "unknown".into(),
            version: "0".into(),
        });
    let mut packages: Vec<LockPackage> = cands
        .iter()
        .map(|c| LockPackage {
            name: c.name.clone(),
            version: c.version.clone(),
            toolchain: c.toolchain.clone(),
            versionsuffix: c.versionsuffix.clone(),
            easyconfig_path: c.easyconfig_path.clone(),
        })
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    StackLock {
        schema_version: 1,
        toolchain,
        generation_label,
        packages,
        solver: SolverMeta {
            engine: engine.into(),
            engine_version: env!("CARGO_PKG_VERSION").into(),
            timestamp: ts,
        },
    }
}

pub fn validate_lock_deps(lock: &StackLock, cands: &[Candidate]) -> Result<(), String> {
    use std::collections::HashMap;
    let by_name: HashMap<&str, &str> = lock
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p.version.as_str()))
        .collect();
    for p in &lock.packages {
        let Some(c) = cands.iter().find(|c| {
            c.name == p.name
                && c.version == p.version
                && c.toolchain.name == p.toolchain.name
                && c.toolchain.version == p.toolchain.version
        }) else {
            continue;
        };
        for (role, deps) in [
            ("dep", c.dependencies.as_slice()),
            ("builddep", c.builddependencies.as_slice()),
        ] {
            for d in deps {
                let Some(v) = by_name.get(d.name.as_str()) else {
                    return Err(format!(
                        "{}={} missing co-selected {role} {}",
                        p.name, p.version, d.name
                    ));
                };
                if !matches_req(v, &d.version_req) {
                    return Err(format!(
                        "{}={} requires {role} {} {} but co-selected {}",
                        p.name, p.version, d.name, d.version_req, v
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_eb_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/gromacs_2025_to_next/easyconfigs")
    }

    #[test]
    fn parse_gromacs_2025b_maps_range_requirements() {
        let p = fixture_eb_root().join("foss-2025b/GROMACS-2025.0-foss-2025b.eb");
        let c = parse_easyconfig_file(&p).expect("parse");
        assert_eq!(c.name, "GROMACS");
        assert_eq!(c.version, "2025.0");
        assert_eq!(c.toolchain.name, "foss");
        assert_eq!(c.toolchain.version, "2025b");
        let mpi = c
            .dependencies
            .iter()
            .find(|d| d.name == "OpenMPI")
            .expect("OpenMPI dep");
        assert_eq!(mpi.version_req, ">=4.1.6");
        let blas = c
            .dependencies
            .iter()
            .find(|d| d.name == "OpenBLAS")
            .expect("OpenBLAS dep");
        assert_eq!(blas.version_req, ">=0.3.27");
    }

    #[test]
    fn exact_pin_maps_to_eq_req() {
        let p = fixture_eb_root().join("foss-2025b/ExactPinDemo-1.0-foss-2025b.eb");
        let c = parse_easyconfig_file(&p).unwrap();
        let mpi = c.dependencies.iter().find(|d| d.name == "OpenMPI").unwrap();
        assert_eq!(mpi.version_req, "==4.1.6");
    }

    #[test]
    fn parse_tree_finds_both_generations() {
        let all = parse_easyconfig_tree(&fixture_eb_root()).expect("tree");
        assert!(all.len() >= 8, "got {}", all.len());
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let next = filter_toolchain(&all, &tc);
        assert!(next
            .iter()
            .any(|c| c.name == "GROMACS" && c.version == "2025.0"));
        assert!(next
            .iter()
            .any(|c| c.name == "OpenMPI" && c.version == "4.1.6"));
    }

    #[test]
    fn strip_comments_preserves_hashes_in_strings() {
        let s = "name = 'foo#bar'\n# comment\nversion = '1.0'\n";
        let t = strip_comments(s);
        assert!(t.contains("foo#bar"));
        assert!(!t.contains("comment"));
    }

    #[test]
    fn parse_builddependencies_separate_from_runtime() {
        let p = fixture_eb_root().join("foss-2025b/BuildDepRoot-1.0-foss-2025b.eb");
        let c = parse_easyconfig_file(&p).expect("parse BuildDepRoot");
        assert_eq!(c.name, "BuildDepRoot");
        assert_eq!(c.version, "1.0");

        let runtime_names: Vec<&str> = c.dependencies.iter().map(|d| d.name.as_str()).collect();
        let build_names: Vec<&str> = c
            .builddependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();

        assert_eq!(runtime_names, vec!["OpenBLAS"]);
        assert_eq!(
            c.dependencies
                .iter()
                .find(|d| d.name == "OpenBLAS")
                .unwrap()
                .version_req,
            ">=0.3.23"
        );
        assert_eq!(build_names, vec!["FFTW"]);
        assert_eq!(
            c.builddependencies
                .iter()
                .find(|d| d.name == "FFTW")
                .unwrap()
                .version_req,
            ">=3.3.10"
        );
        // Roles must not be collapsed into one list.
        assert!(
            !c.dependencies.iter().any(|d| d.name == "FFTW"),
            "FFTW must not appear in runtime dependencies"
        );
        assert!(
            !c.builddependencies.iter().any(|d| d.name == "OpenBLAS"),
            "OpenBLAS must not appear in builddependencies"
        );
    }

    #[test]
    fn runtime_only_easyconfig_has_empty_builddependencies() {
        let p = fixture_eb_root().join("foss-2025b/GROMACS-2025.0-foss-2025b.eb");
        let c = parse_easyconfig_file(&p).expect("parse");
        assert!(!c.dependencies.is_empty());
        assert!(
            c.builddependencies.is_empty(),
            "runtime-only .eb must leave builddependencies empty"
        );
    }

    #[test]
    fn validate_lock_deps_requires_builddependencies() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let root = Candidate {
            name: "Root".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Root-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![DepReq {
                name: "Tool".into(),
                version_req: "==1.0".into(),
            }],
        };
        let tool = Candidate {
            name: "Tool".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Tool-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
        };
        let lock_ok = lock_from_candidates(&[root.clone(), tool.clone()], None, "test");
        assert!(validate_lock_deps(&lock_ok, &[root.clone(), tool.clone()]).is_ok());

        let lock_missing = lock_from_candidates(&[root.clone()], None, "test");
        let err = validate_lock_deps(&lock_missing, &[root, tool]).unwrap_err();
        assert!(
            err.contains("builddep") && err.contains("Tool"),
            "expected builddep failure, got: {err}"
        );
    }
}
