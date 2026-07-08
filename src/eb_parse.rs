//! Parse EasyBuild easyconfig (`.eb`) files into structured candidates.
//!
//! Easyconfigs are a restricted Python DSL. This module evaluates that subset
//! (assignments, lists/tuples/dicts, `SYSTEM`, `local_*` and other name refs)
//! and resolves EasyBuild-style `%(…)s` templates derived from name / version /
//! versionsuffix / toolchain — matching EasyBuild's `EasyConfigParser` plus the
//! core template set used for fixture goldens under `fixtures/parser_hardcases/`.

use crate::domain::{Candidate, DepReq, ExtEntry, LockPackage, SolverMeta, StackLock, Toolchain};
use crate::version::matches_req;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("IO {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("parse {0}: {1}")]
    Parse(String, String),
}

/// One resolved dependency entry (2–4 element EasyBuild dependency tuple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDep {
    pub name: String,
    /// Raw version field after template/local resolution (may be `1.2.3` or `>=1.2`).
    pub version: String,
    #[serde(default)]
    pub versionsuffix: Option<String>,
    /// Per-dependency toolchain override (`None` = inherit the easyconfig toolchain).
    #[serde(default)]
    pub toolchain: Option<Toolchain>,
}

/// One `exts_list` entry after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedExt {
    pub name: String,
    pub version: String,
}

/// Fully resolved easyconfig fields (templates and locals applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEasyconfig {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub versionsuffix: Option<String>,
    pub toolchain: Toolchain,
    #[serde(default)]
    pub dependencies: Vec<ResolvedDep>,
    #[serde(default)]
    pub builddependencies: Vec<ResolvedDep>,
    #[serde(default)]
    pub exts_list: Vec<ResolvedExt>,
    /// Path of the source `.eb` when parsed from disk (empty for in-memory text).
    #[serde(default)]
    pub easyconfig_path: String,
}

impl ResolvedEasyconfig {
    /// Map into the solver-facing [`Candidate`] / [`DepReq`] shapes.
    pub fn to_candidate(&self) -> Candidate {
        Candidate {
            name: self.name.clone(),
            version: self.version.clone(),
            toolchain: self.toolchain.clone(),
            versionsuffix: self.versionsuffix.clone(),
            easyconfig_path: self.easyconfig_path.clone(),
            dependencies: self
                .dependencies
                .iter()
                .map(resolved_dep_to_req)
                .collect(),
            builddependencies: self
                .builddependencies
                .iter()
                .map(resolved_dep_to_req)
                .collect(),
            exts_list: self
                .exts_list
                .iter()
                .map(|e| ExtEntry {
                    name: e.name.clone(),
                    version: e.version.clone(),
                })
                .collect(),
        }
    }
}

fn resolved_dep_to_req(d: &ResolvedDep) -> DepReq {
    DepReq {
        name: d.name.clone(),
        version_req: version_field_to_req(&d.version),
        versionsuffix: d.versionsuffix.clone(),
        toolchain: d.toolchain.clone(),
    }
}

/// Map EasyBuild dependency version field to a solver requirement string.
pub fn version_field_to_req(version: &str) -> String {
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

// --- restricted Python value model ------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    None,
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Dict(Vec<(String, Value)>),
}

impl Value {
    fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn expect_str(&self, ctx: &str) -> Result<String, String> {
        match self {
            Value::Str(s) => Ok(s.clone()),
            Value::Int(i) => Ok(i.to_string()),
            other => Err(format!("{ctx}: expected string, got {other:?}")),
        }
    }
}

fn system_toolchain_value() -> Value {
    Value::Dict(vec![
        ("name".into(), Value::Str("system".into())),
        ("version".into(), Value::Str("system".into())),
    ])
}

// --- comment strip (line-oriented, quote-aware) -----------------------------------

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

// --- recursive-descent expression parser -----------------------------------------

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    env: HashMap<String, Value>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            env: HashMap::new(),
        }
    }

    fn err(&self, msg: impl Into<String>) -> String {
        let msg = msg.into();
        let line = self.src[..self.pos.min(self.src.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count()
            + 1;
        format!("line {line}: {msg}")
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_file(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            self.parse_assignment()?;
        }
        Ok(())
    }

    fn parse_assignment(&mut self) -> Result<(), String> {
        self.skip_ws();
        let name = self.parse_ident()?;
        self.skip_ws();
        if self.bump() != Some(b'=') {
            return Err(self.err(format!("expected '=' after identifier '{name}'")));
        }
        self.skip_ws();
        let val = self.parse_expr()?;
        self.env.insert(name, val);
        // Optional trailing semicolon (rare); ignore commas at top level.
        self.skip_ws();
        if self.peek() == Some(b';') {
            self.pos += 1;
        }
        Ok(())
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        let Some(c0) = self.peek() else {
            return Err(self.err("expected identifier, got EOF"));
        };
        if !(c0.is_ascii_alphabetic() || c0 == b'_') {
            return Err(self.err(format!(
                "expected identifier, got {:?}",
                c0 as char
            )));
        }
        self.pos += 1;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .to_string())
    }

    fn parse_expr(&mut self) -> Result<Value, String> {
        let mut left = self.parse_primary()?;
        // String / value binary ops used in real easyconfigs: `+` concat, `%` format.
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let right = self.parse_primary()?;
                    left = match (left, right) {
                        (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                        (Value::Str(a), Value::Int(b)) => Value::Str(format!("{a}{b}")),
                        (Value::Int(a), Value::Str(b)) => Value::Str(format!("{a}{b}")),
                        (a, b) => {
                            return Err(self.err(format!(
                                "unsupported + operands: {a:?} + {b:?}"
                            )));
                        }
                    };
                }
                Some(b'%') => {
                    self.pos += 1;
                    let right = self.parse_primary()?;
                    left = match (left, right) {
                        (Value::Str(fmt), Value::Str(arg)) => {
                            Value::Str(python_percent_format_one(&fmt, &arg))
                        }
                        (Value::Str(fmt), Value::Int(arg)) => {
                            Value::Str(python_percent_format_one(&fmt, &arg.to_string()))
                        }
                        (a, b) => {
                            return Err(self.err(format!(
                                "unsupported % operands: {a:?} % {b:?}"
                            )));
                        }
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Value, String> {
        self.skip_ws();
        let Some(c) = self.peek() else {
            return Err(self.err("expected expression, got EOF"));
        };
        match c {
            b'\'' | b'"' => self.parse_string(),
            b'[' => self.parse_list(),
            b'(' => self.parse_tuple_or_group(),
            b'{' => self.parse_dict(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.parse_name_or_bool(),
            other => Err(self.err(format!("unexpected char in expression: {other:?}"))),
        }
    }

    fn parse_string(&mut self) -> Result<Value, String> {
        self.skip_ws();
        let quote = self.bump().ok_or_else(|| self.err("expected string"))?;
        // Triple-quoted
        if self.peek() == Some(quote) && self.src.get(self.pos + 1) == Some(&quote) {
            self.pos += 2;
            let start = self.pos;
            while self.pos + 2 < self.src.len() {
                if self.src[self.pos] == quote
                    && self.src[self.pos + 1] == quote
                    && self.src[self.pos + 2] == quote
                {
                    let s = std::str::from_utf8(&self.src[start..self.pos])
                        .map_err(|e| self.err(e.to_string()))?
                        .to_string();
                    self.pos += 3;
                    return Ok(Value::Str(unescape_python_str(&s)));
                }
                self.pos += 1;
            }
            return Err(self.err("unterminated triple-quoted string"));
        }
        let mut out = String::new();
        while let Some(c) = self.bump() {
            if c == quote {
                return Ok(Value::Str(out));
            }
            if c == b'\\' {
                let n = self
                    .bump()
                    .ok_or_else(|| self.err("unterminated string escape"))?;
                out.push(match n {
                    b'n' => '\n',
                    b't' => '\t',
                    b'r' => '\r',
                    b'\\' => '\\',
                    b'\'' => '\'',
                    b'"' => '"',
                    other => other as char,
                });
            } else {
                out.push(c as char);
            }
        }
        Err(self.err("unterminated string"))
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        // Keep floats as strings if dotted (versions are strings in practice).
        if self.peek() == Some(b'.') {
            // Not a pure int — treat remainder as error for numeric; easyconfigs use strings for versions.
            // Allow simple floats as Str for robustness.
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
            return Ok(Value::Str(s.to_string()));
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        let i: i64 = s
            .parse()
            .map_err(|_| self.err(format!("invalid integer {s}")))?;
        Ok(Value::Int(i))
    }

    fn parse_name_or_bool(&mut self) -> Result<Value, String> {
        let name = self.parse_ident()?;
        match name.as_str() {
            "True" => Ok(Value::Bool(true)),
            "False" => Ok(Value::Bool(false)),
            "None" => Ok(Value::None),
            "SYSTEM" => Ok(system_toolchain_value()),
            // EasyBuild built-in source filename constants (need name/version already set).
            "SOURCE_TAR_GZ" | "SOURCELOWER_TAR_GZ" | "SOURCE_TAR_BZ2" | "SOURCELOWER_TAR_BZ2"
            | "SOURCE_TAR_XZ" | "SOURCELOWER_TAR_XZ" => self.eb_source_constant(&name),
            other => {
                if let Some(v) = self.env.get(other) {
                    Ok(v.clone())
                } else {
                    Err(self.err(format!("unknown name '{other}'")))
                }
            }
        }
    }

    fn eb_source_constant(&self, which: &str) -> Result<Value, String> {
        let pkg = self
            .env
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| self.err(format!("{which} requires name = ... first")))?;
        let ver = match self.env.get("version") {
            Some(Value::Str(s)) => s.clone(),
            Some(Value::Int(i)) => i.to_string(),
            _ => return Err(self.err(format!("{which} requires version = ... first"))),
        };
        let base = if which.contains("LOWER") {
            pkg.to_ascii_lowercase()
        } else {
            pkg.to_string()
        };
        let ext = if which.ends_with("BZ2") {
            "tar.bz2"
        } else if which.ends_with("XZ") {
            "tar.xz"
        } else {
            "tar.gz"
        };
        Ok(Value::Str(format!("{base}-{ver}.{ext}")))
    }

    fn parse_list(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'[') {
            return Err(self.err("expected '['"));
        }
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                break;
            }
            items.push(self.parse_expr()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                other => {
                    return Err(self.err(format!(
                        "expected ',' or ']' in list, got {:?}",
                        other.map(|c| c as char)
                    )))
                }
            }
        }
        Ok(Value::List(items))
    }

    fn parse_tuple_or_group(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'(') {
            return Err(self.err("expected '('"));
        }
        self.skip_ws();
        if self.peek() == Some(b')') {
            self.pos += 1;
            return Ok(Value::Tuple(vec![]));
        }
        let first = self.parse_expr()?;
        self.skip_ws();
        if self.peek() == Some(b')') {
            // Single parenthesized expr — treat as bare value (Python group), not 1-tuple,
            // unless a trailing comma was present (handled below).
            self.pos += 1;
            return Ok(first);
        }
        if self.peek() != Some(b',') {
            return Err(self.err("expected ',' or ')' in tuple"));
        }
        self.pos += 1;
        let mut items = vec![first];
        loop {
            self.skip_ws();
            if self.peek() == Some(b')') {
                self.pos += 1;
                break;
            }
            items.push(self.parse_expr()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                other => {
                    return Err(self.err(format!(
                        "expected ',' or ')' in tuple, got {:?}",
                        other.map(|c| c as char)
                    )))
                }
            }
        }
        Ok(Value::Tuple(items))
    }

    fn parse_dict(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'{') {
            return Err(self.err("expected '{'"));
        }
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                break;
            }
            let key_val = self.parse_expr()?;
            let key = key_val
                .expect_str("dict key")
                .map_err(|e| self.err(e))?;
            self.skip_ws();
            if self.bump() != Some(b':') {
                return Err(self.err("expected ':' in dict"));
            }
            let val = self.parse_expr()?;
            items.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                other => {
                    return Err(self.err(format!(
                        "expected ',' or '}}' in dict, got {:?}",
                        other.map(|c| c as char)
                    )))
                }
            }
        }
        Ok(Value::Dict(items))
    }
}

fn unescape_python_str(s: &str) -> String {
    // Triple-quoted bodies are stored raw except common escapes if present.
    s.to_string()
}

/// Minimal Python-style `%s` / `%d` single-arg formatting used in easyconfigs.
fn python_percent_format_one(fmt: &str, arg: &str) -> String {
    if let Some(idx) = fmt.find("%s") {
        let mut out = String::with_capacity(fmt.len() + arg.len());
        out.push_str(&fmt[..idx]);
        out.push_str(arg);
        out.push_str(&fmt[idx + 2..]);
        return out;
    }
    if let Some(idx) = fmt.find("%d") {
        let mut out = String::with_capacity(fmt.len() + arg.len());
        out.push_str(&fmt[..idx]);
        out.push_str(arg);
        out.push_str(&fmt[idx + 2..]);
        return out;
    }
    // No conversion: return format string unchanged (caller may use templates later).
    fmt.to_string()
}

// --- template resolution ---------------------------------------------------------

fn version_part_templates(version: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let parts: Vec<&str> = version.split('.').collect();
    if let Some(major) = parts.first().filter(|p| !p.is_empty()) {
        out.insert("version_major".into(), (*major).to_string());
    }
    if parts.len() > 1 {
        out.insert("version_minor".into(), parts[1].to_string());
        out.insert(
            "version_major_minor".into(),
            format!("{}.{}", parts[0], parts[1]),
        );
    }
    if parts.len() > 2 {
        out.insert("version_patch".into(), parts[2].to_string());
        out.insert(
            "version_minor_patch".into(),
            format!("{}.{}", parts[1], parts[2]),
        );
        out.insert(
            "version_major_minor_patch".into(),
            format!("{}.{}.{}", parts[0], parts[1], parts[2]),
        );
    }
    out
}

fn build_templates(name: &str, version: &str, versionsuffix: &str, tc: &Toolchain) -> HashMap<String, String> {
    let mut tv = HashMap::new();
    tv.insert("name".into(), name.to_string());
    if let Some(ch) = name.chars().next() {
        tv.insert("nameletter".into(), ch.to_string());
    }
    tv.insert("version".into(), version.to_string());
    tv.extend(version_part_templates(version));
    tv.insert("versionsuffix".into(), versionsuffix.to_string());
    tv.insert("toolchain_name".into(), tc.name.clone());
    tv.insert("toolchain_version".into(), tc.version.clone());
    tv
}

fn apply_templates_str(s: &str, templates: &HashMap<String, String>) -> String {
    // EasyBuild uses %(key)s substitution; iterate for rare nested cases.
    let re = regex::Regex::new(r"%\(([^)]+)\)s").expect("template regex");
    let mut cur = s.to_string();
    for _ in 0..8 {
        let mut changed = false;
        let next = re
            .replace_all(&cur, |caps: &regex::Captures| {
                let key = caps.get(1).unwrap().as_str();
                if let Some(v) = templates.get(key) {
                    changed = true;
                    v.clone()
                } else {
                    caps.get(0).unwrap().as_str().to_string()
                }
            })
            .into_owned();
        if !changed || next == cur {
            return next;
        }
        cur = next;
    }
    cur
}

fn apply_templates_value(val: &Value, templates: &HashMap<String, String>) -> Value {
    match val {
        Value::Str(s) => Value::Str(apply_templates_str(s, templates)),
        Value::List(xs) => Value::List(
            xs.iter()
                .map(|x| apply_templates_value(x, templates))
                .collect(),
        ),
        Value::Tuple(xs) => Value::Tuple(
            xs.iter()
                .map(|x| apply_templates_value(x, templates))
                .collect(),
        ),
        Value::Dict(items) => Value::Dict(
            items
                .iter()
                .map(|(k, v)| (k.clone(), apply_templates_value(v, templates)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// --- map Value → domain ----------------------------------------------------------

fn value_to_toolchain(val: &Value, ctx: &str) -> Result<Toolchain, String> {
    match val {
        Value::Dict(items) => {
            let mut name = None;
            let mut version = None;
            for (k, v) in items {
                match k.as_str() {
                    "name" => name = Some(v.expect_str("toolchain.name")?),
                    "version" => version = Some(v.expect_str("toolchain.version")?),
                    _ => {}
                }
            }
            Ok(Toolchain {
                name: name.ok_or_else(|| format!("{ctx}: toolchain missing 'name'"))?,
                version: version.ok_or_else(|| format!("{ctx}: toolchain missing 'version'"))?,
            })
        }
        Value::Tuple(xs) | Value::List(xs) if xs.len() >= 2 => Ok(Toolchain {
            name: xs[0].expect_str("toolchain tuple name")?,
            version: xs[1].expect_str("toolchain tuple version")?,
        }),
        other => Err(format!("{ctx}: unsupported toolchain value {other:?}")),
    }
}

fn value_to_dep(val: &Value) -> Result<ResolvedDep, String> {
    // Filename form: 'OpenMPI-4.1.6-foss-2025b.eb'
    if let Value::Str(s) = val {
        if let Some(dep) = parse_dep_filename(s) {
            return Ok(dep);
        }
        return Err(format!("unsupported string dependency entry: {s}"));
    }
    let items = match val {
        Value::Tuple(xs) | Value::List(xs) => xs,
        other => return Err(format!("unsupported dependency entry: {other:?}")),
    };
    if items.len() < 2 {
        return Err(format!("dependency tuple too short: {items:?}"));
    }
    let name = items[0].expect_str("dep.name")?;
    let version = items[1].expect_str("dep.version")?;
    let mut versionsuffix = None;
    let mut toolchain = None;
    if items.len() >= 3 {
        // Third element is versionsuffix (string); may be empty.
        versionsuffix = Some(items[2].expect_str("dep.versionsuffix")?);
    }
    if items.len() >= 4 {
        toolchain = Some(value_to_toolchain(&items[3], "dep.toolchain")?);
    }
    Ok(ResolvedDep {
        name,
        version,
        versionsuffix,
        toolchain,
    })
}

fn parse_dep_filename(s: &str) -> Option<ResolvedDep> {
    // name-version-toolchain.eb — best-effort for legacy list entries.
    let s = s.strip_suffix(".eb")?;
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() < 3 {
        return None;
    }
    // Heuristic: last two segments often toolchain name + version (foss-2025b).
    let tc_ver = parts[parts.len() - 1];
    let tc_name = parts[parts.len() - 2];
    let name = parts[0];
    let version = parts[1..parts.len() - 2].join("-");
    if version.is_empty() {
        return None;
    }
    let _ = (tc_name, tc_ver);
    Some(ResolvedDep {
        name: name.to_string(),
        version,
        versionsuffix: None,
        toolchain: None,
    })
}

fn value_to_ext(val: &Value) -> Result<ResolvedExt, String> {
    if let Value::Str(s) = val {
        return Ok(ResolvedExt {
            name: s.clone(),
            version: String::new(),
        });
    }
    let items = match val {
        Value::Tuple(xs) | Value::List(xs) => xs,
        other => return Err(format!("unsupported exts_list entry: {other:?}")),
    };
    if items.len() < 2 {
        return Err(format!("exts_list entry too short: {items:?}"));
    }
    Ok(ResolvedExt {
        name: items[0].expect_str("ext.name")?,
        version: items[1].expect_str("ext.version")?,
    })
}

fn value_list_as_slice(val: Option<&Value>) -> Result<&[Value], String> {
    match val {
        None => Ok(&[]),
        Some(Value::List(xs)) => Ok(xs.as_slice()),
        Some(Value::Tuple(xs)) => Ok(xs.as_slice()),
        Some(other) => Err(format!("expected list, got {other:?}")),
    }
}

/// Resolve easyconfig source text to fully expanded fields (no filesystem path).
pub fn resolve_easyconfig_str(src: &str) -> Result<ResolvedEasyconfig, ParseError> {
    let cleaned = strip_comments(src);
    let mut parser = Parser::new(&cleaned);
    parser
        .parse_file()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;

    let name = parser
        .env
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::Parse("<string>".into(), "missing name = ...".into()))?
        .to_string();
    let version = parser
        .env
        .get("version")
        .ok_or_else(|| ParseError::Parse("<string>".into(), "missing version = ...".into()))?
        .expect_str("version")
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;
    let versionsuffix_raw = match parser.env.get("versionsuffix") {
        None => None,
        Some(v) => Some(
            v.expect_str("versionsuffix")
                .map_err(|e| ParseError::Parse("<string>".into(), e))?,
        ),
    };
    let toolchain_val = parser.env.get("toolchain").ok_or_else(|| {
        ParseError::Parse(
            "<string>".into(),
            "missing toolchain = {'name': ..., 'version': ...} or SYSTEM".into(),
        )
    })?;
    let toolchain = value_to_toolchain(toolchain_val, "toolchain")
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;

    let vs_for_templates = versionsuffix_raw.clone().unwrap_or_default();
    let templates = build_templates(&name, &version, &vs_for_templates, &toolchain);

    // Apply templates to fields that may contain %(…)s (including nested deps/exts).
    let name = apply_templates_str(&name, &templates);
    let version = apply_templates_str(&version, &templates);
    let versionsuffix = versionsuffix_raw.map(|s| apply_templates_str(&s, &templates));
    // Rebuild templates if name/version changed (rare for name/version themselves).
    let templates = build_templates(
        &name,
        &version,
        versionsuffix.as_deref().unwrap_or(""),
        &toolchain,
    );

    let deps_val = parser
        .env
        .get("dependencies")
        .map(|v| apply_templates_value(v, &templates));
    let build_val = parser
        .env
        .get("builddependencies")
        .map(|v| apply_templates_value(v, &templates));
    let exts_val = parser
        .env
        .get("exts_list")
        .map(|v| apply_templates_value(v, &templates));

    let dependencies = value_list_as_slice(deps_val.as_ref())
        .map_err(|e| ParseError::Parse("<string>".into(), e))?
        .iter()
        .map(value_to_dep)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;
    let builddependencies = value_list_as_slice(build_val.as_ref())
        .map_err(|e| ParseError::Parse("<string>".into(), e))?
        .iter()
        .map(value_to_dep)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;
    let exts_list = value_list_as_slice(exts_val.as_ref())
        .map_err(|e| ParseError::Parse("<string>".into(), e))?
        .iter()
        .map(value_to_ext)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;

    Ok(ResolvedEasyconfig {
        name,
        version,
        versionsuffix,
        toolchain,
        dependencies,
        builddependencies,
        exts_list,
        easyconfig_path: String::new(),
    })
}

/// Resolve one `.eb` file to fully expanded fields.
pub fn resolve_easyconfig_file(path: &Path) -> Result<ResolvedEasyconfig, ParseError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ParseError::Io(path.display().to_string(), e))?;
    let mut resolved = resolve_easyconfig_str(&raw).map_err(|e| match e {
        ParseError::Parse(_, msg) => ParseError::Parse(path.display().to_string(), msg),
        other => other,
    })?;
    resolved.easyconfig_path = path.display().to_string();
    Ok(resolved)
}

/// Parse one `.eb` file into a solver-facing [`Candidate`].
pub fn parse_easyconfig_file(path: &Path) -> Result<Candidate, ParseError> {
    Ok(resolve_easyconfig_file(path)?.to_candidate())
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
    sort_candidates(&mut out);
    Ok(out)
}

fn candidate_identity_key(c: &Candidate) -> (String, String, String) {
    (c.name.clone(), c.version.clone(), c.toolchain.label())
}

fn sort_candidates(cands: &mut [Candidate]) {
    cands.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.version.cmp(&b.version))
            .then_with(|| a.toolchain.version.cmp(&b.toolchain.version))
            .then_with(|| a.versionsuffix.cmp(&b.versionsuffix))
    });
}

/// Merge candidate layers with **later-layer precedence**: when two candidates
/// share the same name + version + toolchain, the later layer wins (overlay).
///
/// Used for site overlays on top of an upstream easyconfigs tree.
pub fn merge_candidates_with_precedence(layers: &[Vec<Candidate>]) -> Vec<Candidate> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<(String, String, String), Candidate> = BTreeMap::new();
    for layer in layers {
        for c in layer {
            map.insert(candidate_identity_key(c), c.clone());
        }
    }
    let mut out: Vec<Candidate> = map.into_values().collect();
    sort_candidates(&mut out);
    out
}

/// Parse multiple easyconfig trees and merge with later-path precedence.
pub fn parse_easyconfig_trees(roots: &[&Path]) -> Result<Vec<Candidate>, ParseError> {
    let mut layers = Vec::with_capacity(roots.len());
    for root in roots {
        layers.push(parse_easyconfig_tree(root)?);
    }
    Ok(merge_candidates_with_precedence(&layers))
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
    use std::collections::HashMap as Map;
    let by_name: Map<&str, &str> = lock
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

    fn hardcase_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/parser_hardcases")
    }

    fn hardcase_eb(name: &str) -> PathBuf {
        hardcase_root().join("easyconfigs").join(name)
    }

    fn hardcase_golden(name: &str) -> PathBuf {
        hardcase_root().join("resolved").join(name)
    }

    /// Load golden JSON produced by `scripts/resolve_easyconfig_eb.py` (EasyBuild oracle).
    fn load_golden(name: &str) -> ResolvedEasyconfig {
        let path = hardcase_golden(name);
        let raw = std::fs::read_to_string(&path).expect("read golden");
        // Goldens may include `source_easyconfig`; ignore unknown fields via Value filter.
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        if let Some(obj) = v.as_object_mut() {
            obj.remove("source_easyconfig");
            // easyconfig_path not in golden
            obj.insert("easyconfig_path".into(), serde_json::json!(""));
        }
        serde_json::from_value(v).expect("golden shape")
    }

    fn assert_resolved_matches_golden(eb_name: &str, golden_name: &str) {
        let got = resolve_easyconfig_file(&hardcase_eb(eb_name)).expect("resolve");
        let mut expect = load_golden(golden_name);
        // Compare semantic fields; path is set on `got` only.
        expect.easyconfig_path = got.easyconfig_path.clone();
        assert_eq!(got, expect, "mismatch for {eb_name}");
        // No unresolved templates in resolved fields.
        let dump = serde_json::to_string(&got).unwrap();
        assert!(
            !dump.contains("%("),
            "unresolved template left in output: {dump}"
        );
    }

    #[test]
    fn parse_gromacs_2025b_maps_exact_pin_requirements() {
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
        assert_eq!(mpi.version_req, "==5.0.3");
        let blas = c
            .dependencies
            .iter()
            .find(|d| d.name == "OpenBLAS")
            .expect("OpenBLAS dep");
        assert_eq!(blas.version_req, "==0.3.27");
        let py = c
            .dependencies
            .iter()
            .find(|d| d.name == "Python")
            .expect("Python dep (real GROMACS has a hard Python dependency)");
        assert_eq!(py.version_req, "==3.12.3");
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
            "==0.3.27"
        );
        assert_eq!(build_names, vec!["FFTW"]);
        assert_eq!(
            c.builddependencies
                .iter()
                .find(|d| d.name == "FFTW")
                .unwrap()
                .version_req,
            "==3.3.10"
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
                versionsuffix: None,
                toolchain: None,
            }],
            exts_list: vec![],
        };
        let tool = Candidate {
            name: "Tool".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Tool-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
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

    // --- hard-case fixtures vs EasyBuild-captured goldens (no EB at test time) ---

    #[test]
    fn hardcase_templates_version_matches_eb_golden() {
        assert_resolved_matches_golden("templates_version.eb", "templates_version.resolved.json");
    }

    #[test]
    fn hardcase_local_vars_matches_eb_golden() {
        assert_resolved_matches_golden("local_vars.eb", "local_vars.resolved.json");
    }

    #[test]
    fn hardcase_system_toolchain_matches_eb_golden() {
        assert_resolved_matches_golden("system_toolchain.eb", "system_toolchain.resolved.json");
        let r = resolve_easyconfig_file(&hardcase_eb("system_toolchain.eb")).unwrap();
        assert_eq!(r.toolchain.name, "system");
        assert_eq!(r.toolchain.version, "system");
    }

    #[test]
    fn hardcase_multi_element_deps_matches_eb_golden() {
        assert_resolved_matches_golden("multi_element_deps.eb", "multi_element_deps.resolved.json");
        let r = resolve_easyconfig_file(&hardcase_eb("multi_element_deps.eb")).unwrap();
        let dep_b = r.dependencies.iter().find(|d| d.name == "DepB").unwrap();
        assert_eq!(dep_b.version, "1.2");
        assert_eq!(dep_b.versionsuffix.as_deref(), Some("-extra"));
        let dep_c = r.dependencies.iter().find(|d| d.name == "DepC").unwrap();
        assert_eq!(dep_c.version, "9.9.9");
        assert_eq!(
            dep_c.toolchain.as_ref().map(|t| t.label()),
            Some("gompi-2025b".into())
        );
        let dep_d = r.dependencies.iter().find(|d| d.name == "DepD").unwrap();
        assert_eq!(
            dep_d.toolchain.as_ref().map(|t| (t.name.as_str(), t.version.as_str())),
            Some(("system", "system"))
        );
        assert_eq!(r.exts_list.len(), 2);
        assert_eq!(r.exts_list[1].version, "1.0");
    }

    #[test]
    fn hardcase_builddeps_only_matches_eb_golden() {
        assert_resolved_matches_golden("builddeps_only.eb", "builddeps_only.resolved.json");
    }

    #[test]
    fn hardcase_candidate_mapping_preserves_solver_reqs() {
        // Shipped solve path entry: parse_easyconfig_file → Candidate/DepReq.
        let c = parse_easyconfig_file(&hardcase_eb("multi_element_deps.eb")).unwrap();
        assert_eq!(c.name, "MultiDepApp");
        assert_eq!(c.version, "1.2.3");
        assert_eq!(c.versionsuffix.as_deref(), Some("-extra"));
        assert_eq!(c.toolchain.label(), "foss-2025b");
        let a = c.dependencies.iter().find(|d| d.name == "DepA").unwrap();
        assert_eq!(a.version_req, "==1.2.3");
        assert!(a.versionsuffix.is_none());
        assert!(a.toolchain.is_none());
        let b = c.dependencies.iter().find(|d| d.name == "DepB").unwrap();
        assert_eq!(b.version_req, "==1.2");
        assert_eq!(b.versionsuffix.as_deref(), Some("-extra"));
        assert!(b.toolchain.is_none());
        let dep_c = c.dependencies.iter().find(|d| d.name == "DepC").unwrap();
        assert_eq!(dep_c.version_req, "==9.9.9");
        assert_eq!(
            dep_c.toolchain.as_ref().map(|t| t.label()),
            Some("gompi-2025b".into())
        );
        let dep_d = c.dependencies.iter().find(|d| d.name == "DepD").unwrap();
        assert_eq!(
            dep_d.toolchain.as_ref().map(|t| (t.name.as_str(), t.version.as_str())),
            Some(("system", "system"))
        );
        let build = c
            .builddependencies
            .iter()
            .find(|d| d.name == "BuildTool")
            .unwrap();
        assert_eq!(build.version_req, "==1.0");
        assert_eq!(
            build.toolchain.as_ref().map(|t| (t.name.as_str(), t.version.as_str())),
            Some(("system", "system"))
        );
        // exts_list threaded onto the solver-facing candidate.
        assert_eq!(c.exts_list.len(), 2);
        assert_eq!(c.exts_list[0].name, "extpkg");
        assert_eq!(c.exts_list[0].version, "0.1");
        assert_eq!(c.exts_list[1].name, "extpkg2");
        assert_eq!(c.exts_list[1].version, "1.0");
    }

    #[test]
    fn resolved_dep_to_req_threads_versionsuffix_and_toolchain() {
        // Drive the real conversion path via ResolvedEasyconfig::to_candidate.
        let resolved = ResolvedEasyconfig {
            name: "App".into(),
            version: "1.0".into(),
            versionsuffix: None,
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2025b".into(),
            },
            dependencies: vec![
                ResolvedDep {
                    name: "CudaLib".into(),
                    version: "2.0".into(),
                    versionsuffix: Some("-CUDA-12.8".into()),
                    toolchain: None,
                },
                ResolvedDep {
                    name: "SysTool".into(),
                    version: "1.0".into(),
                    versionsuffix: None,
                    toolchain: Some(Toolchain {
                        name: "system".into(),
                        version: "system".into(),
                    }),
                },
            ],
            builddependencies: vec![],
            exts_list: vec![ResolvedExt {
                name: "ext".into(),
                version: "0.1".into(),
            }],
            easyconfig_path: "App.eb".into(),
        };
        let c = resolved.to_candidate();
        let cuda = c.dependencies.iter().find(|d| d.name == "CudaLib").unwrap();
        assert_eq!(cuda.version_req, "==2.0");
        assert_eq!(cuda.versionsuffix.as_deref(), Some("-CUDA-12.8"));
        assert!(cuda.toolchain.is_none());
        let sys = c.dependencies.iter().find(|d| d.name == "SysTool").unwrap();
        assert_eq!(
            sys.toolchain.as_ref().map(|t| t.label()),
            Some("system-system".into())
        );
        assert_eq!(c.exts_list.len(), 1);
        assert_eq!(c.exts_list[0].name, "ext");
        assert_eq!(c.exts_list[0].version, "0.1");
    }

    #[test]
    fn hardcase_tree_parse_uses_shipped_entry_point() {
        let all = parse_easyconfig_tree(&hardcase_root().join("easyconfigs")).expect("tree");
        assert_eq!(all.len(), 5, "expected five hardcase easyconfigs");
        assert!(all.iter().any(|c| c.name == "TemplatedApp"));
        assert!(all.iter().any(|c| c.name == "SystemTool"
            && c.toolchain.name == "system"
            && c.toolchain.version == "system"));
    }

    #[test]
    fn resolve_str_rejects_unresolved_unknown_name() {
        let src = "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\ndependencies = [(missing_var, '1')]\n";
        let err = resolve_easyconfig_str(src).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown name") || msg.contains("missing_var"),
            "{msg}"
        );
    }

    fn repro_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/repro_fixtures")
    }

    /// Real maintainer GROMACS easyconfig: DSL resolve (not bump/rewrite) must
    /// surface exact deps including Python.
    #[test]
    fn resolve_real_gromacs_repro_fixture() {
        let p = repro_root().join("gromacs/GROMACS-2024.4-foss-2024a.eb");
        let r = resolve_easyconfig_file(&p).expect("resolve real GROMACS");
        assert_eq!(r.name, "GROMACS");
        assert_eq!(r.version, "2024.4");
        assert_eq!(r.toolchain.name, "foss");
        assert_eq!(r.toolchain.version, "2024a");
        let py = r
            .dependencies
            .iter()
            .find(|d| d.name == "Python")
            .expect("Python hard dep");
        assert_eq!(py.version, "3.12.3");
        // Exact co-pins (no operator in source → solver sees ==).
        let c = r.to_candidate();
        let py_req = c
            .dependencies
            .iter()
            .find(|d| d.name == "Python")
            .unwrap();
        assert_eq!(py_req.version_req, "==3.12.3");
        let scipy = c
            .dependencies
            .iter()
            .find(|d| d.name == "SciPy-bundle")
            .unwrap();
        assert_eq!(scipy.version_req, "==2024.05");
        // Build deps also exact.
        let cmake = c
            .builddependencies
            .iter()
            .find(|d| d.name == "CMake")
            .unwrap();
        assert_eq!(cmake.version_req, "==3.29.3");
        // gmxapi extension from exts_list.
        assert!(
            r.exts_list.iter().any(|e| e.name == "gmxapi"),
            "expected gmxapi in exts_list: {:?}",
            r.exts_list
        );
    }

    #[test]
    fn parse_real_fiona_and_mdtraj_repro_fixtures() {
        let fiona = parse_easyconfig_file(&repro_root().join("fiona/Fiona-1.10.1-foss-2024a.eb"))
            .expect("parse Fiona");
        assert_eq!(fiona.name, "Fiona");
        assert_eq!(fiona.version, "1.10.1");
        assert_eq!(fiona.toolchain.label(), "foss-2024a");
        let py = fiona
            .dependencies
            .iter()
            .find(|d| d.name == "Python")
            .unwrap();
        assert_eq!(py.version_req, "==3.12.3");
        assert_eq!(
            fiona
                .dependencies
                .iter()
                .find(|d| d.name == "GDAL")
                .unwrap()
                .version_req,
            "==3.10.0"
        );
        assert!(
            fiona.exts_list.iter().any(|e| e.name == "cligj"),
            "Fiona bundles cligj: {:?}",
            fiona.exts_list
        );

        let md = parse_easyconfig_file(&repro_root().join("mdtraj/MDTraj-1.10.3-foss-2024a.eb"))
            .expect("parse MDTraj");
        assert_eq!(md.name, "MDTraj");
        assert_eq!(md.version, "1.10.3");
        assert_eq!(
            md.dependencies
                .iter()
                .find(|d| d.name == "Python")
                .unwrap()
                .version_req,
            "==3.12.3"
        );
        assert_eq!(
            md.dependencies
                .iter()
                .find(|d| d.name == "SciPy-bundle")
                .unwrap()
                .version_req,
            "==2024.05"
        );
    }

    #[test]
    fn parse_real_pulp_with_source_tar_gz_constant() {
        let c = parse_easyconfig_file(&repro_root().join("pulp/PuLP-2.8.0-foss-2024a.eb"))
            .expect("parse PuLP (uses SOURCE_TAR_GZ)");
        assert_eq!(c.name, "PuLP");
        assert_eq!(c.version, "2.8.0");
        assert_eq!(c.toolchain.label(), "foss-2024a");
        assert_eq!(
            c.dependencies
                .iter()
                .find(|d| d.name == "Python")
                .unwrap()
                .version_req,
            "==3.12.3"
        );
        assert_eq!(
            c.dependencies
                .iter()
                .find(|d| d.name == "Cbc")
                .unwrap()
                .version_req,
            "==2.10.12"
        );
    }

    #[test]
    fn merge_candidates_overlay_wins_same_identity() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let upstream = vec![
            Candidate {
                name: "Lib".into(),
                version: "1.0".into(),
                toolchain: tc.clone(),
                versionsuffix: None,
                easyconfig_path: "upstream/Lib-1.0.eb".into(),
                dependencies: vec![],
                builddependencies: vec![],
                exts_list: vec![],
            },
            Candidate {
                name: "Keep".into(),
                version: "2.0".into(),
                toolchain: tc.clone(),
                versionsuffix: None,
                easyconfig_path: "upstream/Keep-2.0.eb".into(),
                dependencies: vec![],
                builddependencies: vec![],
                exts_list: vec![],
            },
        ];
        let overlay = vec![Candidate {
            name: "Lib".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "overlay/Lib-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        }];
        let merged = merge_candidates_with_precedence(&[upstream, overlay]);
        assert_eq!(merged.len(), 2);
        let lib = merged.iter().find(|c| c.name == "Lib").unwrap();
        assert_eq!(lib.easyconfig_path, "overlay/Lib-1.0.eb");
        assert!(
            merged
                .iter()
                .any(|c| c.name == "Keep" && c.easyconfig_path.contains("upstream")),
            "non-overridden upstream must remain"
        );
    }
}
