//! Parse EasyBuild easyconfig (`.eb`) files into structured candidates.
//!
//! Easyconfigs are a restricted Python DSL. This module evaluates that subset
//! (assignments, lists/tuples/dicts, `SYSTEM`, `local_*` and other name refs)
//! and resolves EasyBuild-style `%(…)s` templates derived from name / version /
//! versionsuffix / toolchain — matching EasyBuild's `EasyConfigParser` plus the
//! core template set used for fixture goldens under `fixtures/parser_hardcases/`.

use crate::domain::{Candidate, DepReq, ExtEntry, LockPackage, SolverMeta, StackLock, Toolchain};
use crate::eb_template_constants::EB_TEMPLATE_CONSTANTS;
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
///
/// Solver-facing co-selection uses [`Self::to_candidate`]. Packaging /
/// contribution checks also use the optional metadata fields below
/// (`easyblock`, `configopts`, `moduleclass`, …).
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
    /// EasyBuild easyblock class name (`MesonNinja`, `CMakeMake`, …).
    #[serde(default)]
    pub easyblock: Option<String>,
    /// Meson/CMake/configure flags string after template expansion.
    #[serde(default)]
    pub configopts: Option<String>,
    /// EasyBuild moduleclass (`chem`, `lib`, `tools`, …).
    #[serde(default)]
    pub moduleclass: Option<String>,
    /// Homepage URL when set.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Source checksums list (strings), when present — used for packaging gates.
    #[serde(default)]
    pub checksums: Vec<String>,
    /// Number of entries in `sources` (0 when the field is absent).
    #[serde(default)]
    pub sources_count: usize,
    /// Patch file names from `patches` (tuple/dict entries reduced to the name).
    #[serde(default)]
    pub patch_names: Vec<String>,
    /// Per-`checksums`-LIST-ENTRY dict keys (empty inner vec for plain-string
    /// entries). Parallel to the checksums list entries, not the flattened
    /// values, so a multi-arch dict entry stays one entry.
    #[serde(default)]
    pub checksum_entry_keys: Vec<Vec<String>>,
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
            dependencies: self.dependencies.iter().map(resolved_dep_to_req).collect(),
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
        let mut env = HashMap::new();
        // Seed full EasyBuild TEMPLATE_CONSTANTS (%(…)s applied later).
        for (name, value) in EB_TEMPLATE_CONSTANTS {
            env.insert((*name).to_string(), Value::Str((*value).to_string()));
        }
        Self {
            src: src.as_bytes(),
            pos: 0,
            env,
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
        // Tolerant: extract every assignment we can; skip unmodeled statements
        // (if/for/configopts expressions, etc.) so required fields still parse.
        loop {
            self.skip_ws();
            if self.pos >= self.src.len() {
                break;
            }
            if self.at_control_keyword() {
                self.skip_compound_or_line();
                continue;
            }
            let start = self.pos;
            match self.parse_assignment() {
                Ok(()) => {}
                Err(_) => {
                    self.pos = start;
                    if !self.skip_one_statement() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn at_control_keyword(&self) -> bool {
        const KWS: &[&[u8]] = &[
            b"if",
            b"for",
            b"while",
            b"try",
            b"with",
            b"def",
            b"class",
            b"else",
            b"elif",
            b"except",
            b"finally",
            b"async",
            b"raise",
            b"return",
            b"assert",
            b"import",
            b"from",
            b"pass",
            b"break",
            b"continue",
            b"global",
            b"nonlocal",
            b"del",
            b"yield",
            b"lambda",
        ];
        let rest = &self.src[self.pos..];
        for kw in KWS {
            if rest.starts_with(kw) {
                let after = self.pos + kw.len();
                let next = self.src.get(after).copied();
                // keyword boundary: not part of a longer identifier
                if next
                    .map(|c| c.is_ascii_alphanumeric() || c == b'_')
                    .unwrap_or(false)
                {
                    continue;
                }
                return true;
            }
        }
        false
    }

    /// Skip a compound statement (header line + indented body) or a single line.
    fn skip_compound_or_line(&mut self) {
        // Consume until end of logical line (bracket-aware).
        let _ = self.skip_one_statement();
        // Skip following indented body lines (Python-style).
        loop {
            let saved = self.pos;
            // Count leading spaces/tabs on the next line.
            if self.pos >= self.src.len() {
                break;
            }
            if self.peek() == Some(b'\n') {
                self.pos += 1;
            }
            let mut i = self.pos;
            let mut indent = 0usize;
            while let Some(&c) = self.src.get(i) {
                if c == b' ' {
                    indent += 1;
                    i += 1;
                } else if c == b'\t' {
                    indent += 4;
                    i += 1;
                } else {
                    break;
                }
            }
            if indent == 0 {
                self.pos = saved;
                // re-consume newline we may have stepped past only if body empty
                break;
            }
            // Blank indented line: continue.
            if self.src.get(i) == Some(&b'\n') {
                self.pos = i + 1;
                continue;
            }
            self.pos = i;
            let _ = self.skip_one_statement();
        }
    }

    /// Advance past one statement (assignment, expression, etc.) with bracket/string depth.
    /// Returns false if no progress was made.
    fn skip_one_statement(&mut self) -> bool {
        let start = self.pos;
        if self.pos >= self.src.len() {
            return false;
        }
        let mut depth_paren = 0i32;
        let mut depth_brack = 0i32;
        let mut depth_brace = 0i32;
        let mut in_s = false;
        let mut in_d = false;
        let mut escape = false;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if escape {
                escape = false;
                self.pos += 1;
                continue;
            }
            if in_s {
                if c == b'\\' {
                    escape = true;
                } else if c == b'\'' {
                    in_s = false;
                }
                self.pos += 1;
                continue;
            }
            if in_d {
                if c == b'\\' {
                    escape = true;
                } else if c == b'"' {
                    in_d = false;
                }
                self.pos += 1;
                continue;
            }
            match c {
                b'\'' => in_s = true,
                b'"' => in_d = true,
                b'(' => depth_paren += 1,
                b')' => depth_paren = (depth_paren - 1).max(0),
                b'[' => depth_brack += 1,
                b']' => depth_brack = (depth_brack - 1).max(0),
                b'{' => depth_brace += 1,
                b'}' => depth_brace = (depth_brace - 1).max(0),
                b'\n' if depth_paren == 0 && depth_brack == 0 && depth_brace == 0 => {
                    self.pos += 1;
                    break;
                }
                b'#' if depth_paren == 0 && depth_brack == 0 && depth_brace == 0 => {
                    // rest of line is comment
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    if self.pos < self.src.len() {
                        self.pos += 1;
                    }
                    break;
                }
                _ => {}
            }
            self.pos += 1;
        }
        self.pos > start
    }

    fn parse_assignment(&mut self) -> Result<(), String> {
        self.skip_ws();
        let name = self.parse_ident()?;
        self.skip_ws();
        // Support `=` and `+=` (string append used in real easyconfigs).
        let aug_add = if self.peek() == Some(b'+') && self.src.get(self.pos + 1) == Some(&b'=') {
            self.pos += 2;
            true
        } else if self.bump() == Some(b'=') {
            false
        } else {
            return Err(self.err(format!("expected '=' after identifier '{name}'")));
        };
        self.skip_ws();
        let val = self.parse_expr()?;
        if aug_add {
            match (self.env.get(&name).cloned(), val) {
                (Some(Value::Str(a)), Value::Str(b)) => {
                    self.env.insert(name, Value::Str(a + &b));
                }
                (_, v) => {
                    self.env.insert(name, v);
                }
            }
        } else {
            self.env.insert(name, val);
        }
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
            return Err(self.err(format!("expected identifier, got {:?}", c0 as char)));
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
                            return Err(self.err(format!("unsupported + operands: {a:?} + {b:?}")));
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
                            return Err(self.err(format!("unsupported % operands: {a:?} % {b:?}")));
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
            other => {
                if let Some(v) = self.env.get(other) {
                    Ok(v.clone())
                } else {
                    Err(self.err(format!("unknown name '{other}'")))
                }
            }
        }
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
            let key = key_val.expect_str("dict key").map_err(|e| self.err(e))?;
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

fn build_templates(
    name: &str,
    version: &str,
    versionsuffix: &str,
    tc: &Toolchain,
) -> HashMap<String, String> {
    let mut tv = HashMap::new();
    tv.insert("name".into(), name.to_string());
    let namelower = name.to_ascii_lowercase();
    tv.insert("namelower".into(), namelower.clone());
    if let Some(ch) = name.chars().next() {
        tv.insert("nameletter".into(), ch.to_string());
        tv.insert(
            "nameletterlower".into(),
            ch.to_ascii_lowercase().to_string(),
        );
    }
    // Defaults used by GITHUB_*/BITBUCKET_* constants when not set in the recipe.
    tv.insert("github_account".into(), namelower.clone());
    tv.insert("bitbucket_account".into(), namelower);
    tv.insert("version".into(), version.to_string());
    tv.extend(version_part_templates(version));
    tv.insert("versionsuffix".into(), versionsuffix.to_string());
    tv.insert("toolchain_name".into(), tc.name.clone());
    tv.insert("toolchain_version".into(), tc.version.clone());
    // pyshortver is used in sanity paths; approximate from version major.minor when possible.
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        tv.insert("pyshortver".into(), format!("{}.{}", parts[0], parts[1]));
    }
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

    let easyblock = opt_str_field(&parser.env, "easyblock", &templates);
    let configopts = opt_str_field(&parser.env, "configopts", &templates);
    let moduleclass = opt_str_field(&parser.env, "moduleclass", &templates);
    let homepage = opt_str_field(&parser.env, "homepage", &templates);
    let checksums = opt_str_list_field(&parser.env, "checksums", &templates);
    let sources_count = env_list_len(&parser.env, "sources", &templates);
    let patch_names = patch_names_field(&parser.env, &templates);
    let checksum_entry_keys = checksum_entry_keys_field(&parser.env, &templates);

    Ok(ResolvedEasyconfig {
        name,
        version,
        versionsuffix,
        toolchain,
        dependencies,
        builddependencies,
        exts_list,
        easyconfig_path: String::new(),
        easyblock,
        configopts,
        moduleclass,
        homepage,
        checksums,
        sources_count,
        patch_names,
        checksum_entry_keys,
    })
}

/// Length of a list-valued field after template expansion (0 when absent or
/// not a list).
fn env_list_len(
    env: &HashMap<String, Value>,
    key: &str,
    templates: &HashMap<String, String>,
) -> usize {
    env.get(key)
        .map(|v| apply_templates_value(v, templates))
        .and_then(|v| value_list_as_slice(Some(&v)).ok().map(|l| l.len()))
        .unwrap_or(0)
}

/// Patch names from `patches`: plain string, `(name, level)` tuple/list, or a
/// dict with a `name`/`filename` key. Unrecognised entries are skipped.
fn patch_names_field(
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
) -> Vec<String> {
    let Some(v) = env.get("patches") else {
        return Vec::new();
    };
    let v = apply_templates_value(v, templates);
    let Ok(items) = value_list_as_slice(Some(&v)) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Str(s) => Some(s.clone()),
            Value::Tuple(xs) | Value::List(xs) => {
                xs.first().and_then(|x| x.as_str()).map(str::to_string)
            }
            Value::Dict(kvs) => kvs
                .iter()
                .find(|(k, _)| k == "name" || k == "filename")
                .and_then(|(_, val)| val.as_str())
                .map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Dict keys per `checksums` list entry (empty vec for plain-string entries).
fn checksum_entry_keys_field(
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
) -> Vec<Vec<String>> {
    let Some(v) = env.get("checksums") else {
        return Vec::new();
    };
    let v = apply_templates_value(v, templates);
    let Ok(items) = value_list_as_slice(Some(&v)) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| match item {
            Value::Dict(kvs) => kvs.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// Structural findings for the `checksums` list (EasyBuild convention:
/// positional, all `sources` entries first, then `patches`). Catches the
/// class of failure where a patch checksum is inserted in a source slot,
/// which otherwise only surfaces as an eb "Missing checksum for X" abort
/// after a build cycle has already been spent.
pub fn checksum_structure_findings(recipe: &ResolvedEasyconfig) -> Vec<String> {
    let mut out = Vec::new();
    let entries = recipe.checksum_entry_keys.len();
    if entries == 0 {
        return out;
    }
    let expected = recipe.sources_count + recipe.patch_names.len();
    if recipe.sources_count > 0 && entries != expected {
        out.push(format!(
            "checksums has {entries} entries but sources ({}) + patches ({}) = {expected} \
             (EasyBuild matches checksums positionally: sources first, then patches)",
            recipe.sources_count,
            recipe.patch_names.len(),
        ));
    }
    for (i, keys) in recipe.checksum_entry_keys.iter().enumerate() {
        if i >= recipe.sources_count {
            break;
        }
        for k in keys {
            if recipe.patch_names.iter().any(|p| p == k) {
                out.push(format!(
                    "checksum entry {i} is keyed by patch '{k}' but sits in a source slot \
                     (positions 0..{} are sources; move patch checksums after all source entries)",
                    recipe.sources_count,
                ));
            }
        }
    }
    out
}

fn opt_str_field(
    env: &HashMap<String, Value>,
    key: &str,
    templates: &HashMap<String, String>,
) -> Option<String> {
    env.get(key).and_then(|v| {
        let v = apply_templates_value(v, templates);
        v.expect_str(key).ok()
    })
}

fn opt_str_list_field(
    env: &HashMap<String, Value>,
    key: &str,
    templates: &HashMap<String, String>,
) -> Vec<String> {
    let Some(v) = env.get(key) else {
        return Vec::new();
    };
    let v = apply_templates_value(v, templates);
    match value_list_as_slice(Some(&v)) {
        Ok(items) => items.iter().flat_map(checksum_strings_from_value).collect(),
        Err(_) => match v.expect_str(key) {
            Ok(s) => vec![s],
            Err(_) => Vec::new(),
        },
    }
}

/// EasyBuild checksums may be plain strings or one-key dicts
/// `{'file.tar.gz': 'sha256…'}`. Packaging gates need the sha256 tokens.
fn checksum_strings_from_value(v: &Value) -> Vec<String> {
    match v {
        Value::Str(s) => vec![s.clone()],
        Value::Dict(items) => items
            .iter()
            .filter_map(|(_, val)| val.expect_str("checksum").ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve one `.eb` file to fully expanded fields.
pub fn resolve_easyconfig_file(path: &Path) -> Result<ResolvedEasyconfig, ParseError> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| ParseError::Io(path.display().to_string(), e))?;
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

/// One easyconfig path that could not be parsed into a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEasyconfig {
    pub path: String,
    pub error: String,
}

/// Result of walking an easyconfig tree: successes + skipped unparseable files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParseTreeResult {
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<SkippedEasyconfig>,
}

impl ParseTreeResult {
    pub fn skip_count(&self) -> usize {
        self.skipped.len()
    }

    pub fn parsed_count(&self) -> usize {
        self.candidates.len()
    }

    /// Coverage fraction `parsed / (parsed + skipped)`; 1.0 when the tree is empty.
    pub fn coverage(&self) -> f64 {
        let p = self.parsed_count();
        let s = self.skip_count();
        if p + s == 0 {
            1.0
        } else {
            p as f64 / (p + s) as f64
        }
    }

    /// Merge another result (later candidates override on identity; skips append).
    pub fn merge_with_precedence(mut self, other: ParseTreeResult) -> ParseTreeResult {
        let layers = vec![self.candidates, other.candidates];
        self.candidates = merge_candidates_with_precedence(&layers);
        self.skipped.extend(other.skipped);
        self
    }
}

/// Walk a directory tree for `*.eb` and parse all easyconfigs.
///
/// Unparseable files are **skipped** (not fatal): they appear in
/// [`ParseTreeResult::skipped`] so callers can report coverage without
/// aborting a real multi-thousand-file tree on the first bad recipe.
pub fn parse_easyconfig_tree(root: &Path) -> Result<ParseTreeResult, ParseError> {
    let mut out = ParseTreeResult::default();
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
                match parse_easyconfig_file(&p) {
                    Ok(c) => out.candidates.push(c),
                    Err(e) => out.skipped.push(SkippedEasyconfig {
                        path: p.display().to_string(),
                        error: e.to_string(),
                    }),
                }
            }
        }
    }
    sort_candidates(&mut out.candidates);
    out.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// One missing (or unmatched) dependency from a packaging/robot check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingDep {
    pub name: String,
    pub version: String,
    pub versionsuffix: Option<String>,
    pub toolchain: Option<Toolchain>,
    /// Runtime vs build-time role in the recipe.
    pub role: String,
    pub reason: String,
}

/// Result of checking that a recipe's deps exist somewhere in a robot universe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeDepCheck {
    pub recipe: String,
    pub name: String,
    pub version: String,
    pub toolchain: Toolchain,
    pub easyblock: Option<String>,
    pub configopts: Option<String>,
    pub moduleclass: Option<String>,
    pub checksum_count: usize,
    pub missing: Vec<MissingDep>,
    pub found: Vec<String>,
}

impl RecipeDepCheck {
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Whether a universe candidate satisfies a resolved dep (name + version +
/// optional versionsuffix + optional per-dep toolchain). Cross-toolchain
/// dependencies crossing from a parent toolchain to a core toolchain are first-class here —
/// unlike [`filter_toolchain`] which keeps only the policy toolchain.
///
/// When `dep.toolchain` is **None**, any toolchain with a matching
/// name/version/suffix is accepted (legacy identity match). Prefer
/// [`candidate_matches_dep_for_recipe`] so unpinned deps must live in the
/// recipe generation hierarchy (closer to EasyBuild robot behaviour).
pub fn candidate_matches_dep(c: &Candidate, dep: &ResolvedDep) -> bool {
    candidate_matches_dep_core(c, dep, /*require_hierarchy*/ None)
}

/// Like [`candidate_matches_dep`], but when the dep has **no** explicit
/// toolchain pin, only candidates whose toolchain is a member of `hierarchy`
/// count (e.g. CapnProto on GCCcore-14.x does not satisfy foss-2026.1 which
/// needs GCCcore-15.2.0). Explicit fourth-tuple pins still match exactly,
/// including explicitly selected cross-generation dependencies.
pub fn candidate_matches_dep_for_recipe(
    c: &Candidate,
    dep: &ResolvedDep,
    hierarchy: &crate::hierarchy::ToolchainHierarchy,
) -> bool {
    candidate_matches_dep_core(c, dep, Some(hierarchy))
}

fn candidate_matches_dep_core(
    c: &Candidate,
    dep: &ResolvedDep,
    hierarchy: Option<&crate::hierarchy::ToolchainHierarchy>,
) -> bool {
    if c.name != dep.name {
        return false;
    }
    if !matches_req(&c.version, &version_field_to_req(&dep.version)) {
        return false;
    }
    let want_vs = dep.versionsuffix.as_deref().unwrap_or("");
    let got_vs = c.versionsuffix.as_deref().unwrap_or("");
    if want_vs != got_vs {
        return false;
    }
    if let Some(tc) = &dep.toolchain {
        return crate::hierarchy::toolchains_match(&c.toolchain, tc);
    }
    // Unpinned: EasyBuild resolves within the parent recipe hierarchy.
    if let Some(h) = hierarchy {
        return h.contains(&c.toolchain);
    }
    true
}

/// Check that every runtime/build dep of `recipe` appears as a candidate in
/// `universe` (any tree layer already merged). Does **not** run the SAT
/// solver — this is the packaging/robot completeness gate used before `eb`.
///
/// Unpinned deps must match a hierarchy member of the recipe toolchain
/// (derived from the robot universe when possible), so an older-generation
/// GCCcore candidate does not false-pass a newer foss recipe.
pub fn check_recipe_deps(recipe: &ResolvedEasyconfig, universe: &[Candidate]) -> RecipeDepCheck {
    let hierarchy =
        crate::hierarchy::hierarchy_for_with_tree(&recipe.toolchain, None, universe).ok();
    let mut missing = Vec::new();
    let mut found = Vec::new();
    for (role, deps) in [
        ("runtime", recipe.dependencies.as_slice()),
        ("build", recipe.builddependencies.as_slice()),
    ] {
        for d in deps {
            let matched = universe.iter().find(|c| {
                if let Some(ref h) = hierarchy {
                    candidate_matches_dep_for_recipe(c, d, h)
                } else {
                    candidate_matches_dep(c, d)
                }
            });
            if let Some(c) = matched {
                found.push(format!(
                    "{role}:{}-{}{}",
                    d.name,
                    d.version,
                    if d.toolchain.is_some() {
                        d.toolchain
                            .as_ref()
                            .map(|t| format!(" ({})", t.label()))
                            .unwrap_or_default()
                    } else {
                        format!(" ({})", c.toolchain.label())
                    }
                ));
            } else {
                let hint = crate::hierarchy::nearest_candidates_hint(&d.name, universe);
                let hier_note = hierarchy
                    .as_ref()
                    .map(|h| {
                        format!(
                            " (need hierarchy member of {}-{}: {})",
                            recipe.toolchain.name,
                            recipe.toolchain.version,
                            h.member_labels().join(" < ")
                        )
                    })
                    .unwrap_or_default();
                missing.push(MissingDep {
                    name: d.name.clone(),
                    version: d.version.clone(),
                    versionsuffix: d.versionsuffix.clone(),
                    toolchain: d.toolchain.clone(),
                    role: role.into(),
                    reason: format!(
                        "no candidate for {} {}{} in robot universe{hier_note}{hint}",
                        d.name,
                        d.version,
                        d.toolchain
                            .as_ref()
                            .map(|t| format!(" toolchain={}", t.label()))
                            .unwrap_or_default()
                    ),
                });
            }
        }
    }
    // Structural checksum findings fail the gate too: a patch checksum in a
    // source slot only surfaces from eb itself after a wasted build cycle.
    for finding in checksum_structure_findings(recipe) {
        missing.push(MissingDep {
            name: "checksums".into(),
            version: String::new(),
            versionsuffix: None,
            toolchain: None,
            role: "packaging".into(),
            reason: finding,
        });
    }
    RecipeDepCheck {
        recipe: recipe.easyconfig_path.clone(),
        name: recipe.name.clone(),
        version: recipe.version.clone(),
        toolchain: recipe.toolchain.clone(),
        easyblock: recipe.easyblock.clone(),
        configopts: recipe.configopts.clone(),
        moduleclass: recipe.moduleclass.clone(),
        checksum_count: recipe.checksums.len(),
        missing,
        found,
    }
}

/// Packaging gate: checksums present, moduleclass set, and optional required
/// configopts substrings (e.g. `-Dwith_tests=false`).
///
/// A missing `easyblock` is **not** an error: EasyBuild derives the easyblock
/// from the software name when the recipe omits it (`OpenMPI` -> `EB_OpenMPI`,
/// `GCC` -> `EB_GCC`, ...), which is how the majority of upstream recipes are
/// written. Only recipes whose name does not map to a software-specific
/// easyblock need to declare one, and the recipe author — not this gate —
/// makes that call.
pub fn packaging_gate(
    recipe: &ResolvedEasyconfig,
    required_configopts: &[&str],
) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    if recipe.moduleclass.as_deref().unwrap_or("").is_empty() {
        errs.push("missing moduleclass".into());
    }
    if recipe.checksums.is_empty() {
        errs.push("missing checksums".into());
    }
    let opts = recipe.configopts.as_deref().unwrap_or("");
    for need in required_configopts {
        if !opts.contains(need) {
            errs.push(format!("configopts missing required flag {need:?}"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// EasyBuild-style letter directory for `name` (`ExamplePkg` → `e`).
pub fn easyconfig_letter_dir(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_ascii_lowercase().to_string())
        .unwrap_or_else(|| "x".into())
}

/// Conventional basename: `Name-version-Toolchain-tcver.eb` (+ versionsuffix).
pub fn easyconfig_basename(
    name: &str,
    version: &str,
    tc: &Toolchain,
    versionsuffix: Option<&str>,
) -> String {
    let vs = versionsuffix.unwrap_or("");
    if is_system_toolchain(tc) {
        format!("{name}-{version}{vs}.eb")
    } else {
        format!("{name}-{version}-{0}-{1}{vs}.eb", tc.name, tc.version)
    }
}

fn is_system_toolchain(tc: &Toolchain) -> bool {
    tc.name.eq_ignore_ascii_case("system")
}

fn candidate_identity_key(c: &Candidate) -> (String, String, String, String) {
    (
        c.name.clone(),
        c.version.clone(),
        c.toolchain.label(),
        c.versionsuffix.clone().unwrap_or_default(),
    )
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
/// share the same name + version + toolchain + versionsuffix, the later layer
/// wins (overlay). Distinct installable variants remain separate candidates.
///
/// Used for site overlays on top of an upstream easyconfigs tree.
pub fn merge_candidates_with_precedence(layers: &[Vec<Candidate>]) -> Vec<Candidate> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<(String, String, String, String), Candidate> = BTreeMap::new();
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
/// Skipped paths from every tree are retained.
pub fn parse_easyconfig_trees(roots: &[&Path]) -> Result<ParseTreeResult, ParseError> {
    let mut acc = ParseTreeResult::default();
    let mut layers: Vec<Vec<Candidate>> = Vec::with_capacity(roots.len());
    for root in roots {
        let r = parse_easyconfig_tree(root)?;
        layers.push(r.candidates);
        acc.skipped.extend(r.skipped);
    }
    acc.candidates = merge_candidates_with_precedence(&layers);
    acc.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(acc)
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
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
        let mut got = resolve_easyconfig_file(&hardcase_eb(eb_name)).expect("resolve");
        let mut expect = load_golden(golden_name);
        // Compare semantic fields; path is set on `got` only.
        expect.easyconfig_path = got.easyconfig_path.clone();
        // Packaging metadata (easyblock/configopts/…) is additive; goldens predate it.
        got.easyblock = expect.easyblock.clone();
        got.configopts = expect.configopts.clone();
        got.moduleclass = expect.moduleclass.clone();
        got.homepage = expect.homepage.clone();
        got.checksums = expect.checksums.clone();
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
        let all = parse_easyconfig_tree(&fixture_eb_root())
            .expect("tree")
            .candidates;
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

        let lock_missing = lock_from_candidates(std::slice::from_ref(&root), None, "test");
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
            dep_d
                .toolchain
                .as_ref()
                .map(|t| (t.name.as_str(), t.version.as_str())),
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
            dep_d
                .toolchain
                .as_ref()
                .map(|t| (t.name.as_str(), t.version.as_str())),
            Some(("system", "system"))
        );
        let build = c
            .builddependencies
            .iter()
            .find(|d| d.name == "BuildTool")
            .unwrap();
        assert_eq!(build.version_req, "==1.0");
        assert_eq!(
            build
                .toolchain
                .as_ref()
                .map(|t| (t.name.as_str(), t.version.as_str())),
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
            easyblock: None,
            configopts: None,
            moduleclass: None,
            homepage: None,
            checksums: vec![],
            sources_count: 0,
            patch_names: vec![],
            checksum_entry_keys: vec![],
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
        let all = parse_easyconfig_tree(&hardcase_root().join("easyconfigs"))
            .expect("tree")
            .candidates;
        assert_eq!(all.len(), 5, "expected five hardcase easyconfigs");
        assert!(all.iter().any(|c| c.name == "TemplatedApp"));
        assert!(all.iter().any(|c| c.name == "SystemTool"
            && c.toolchain.name == "system"
            && c.toolchain.version == "system"));
    }

    #[test]
    fn resolve_str_tolerates_unknown_name_in_noncritical_assignment() {
        // Unmodeled / broken assignments are skipped; required fields still parse.
        let src = "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\nconfigopts = missing_var + 'x'\ndependencies = []\n";
        let r = resolve_easyconfig_str(src).expect("tolerant resolve");
        assert_eq!(r.name, "X");
        assert_eq!(r.version, "1");
        assert!(r.dependencies.is_empty());
    }

    #[test]
    fn resolve_str_rejects_missing_required_name() {
        let src = "version = '1'\ntoolchain = SYSTEM\n";
        let err = resolve_easyconfig_str(src).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing name"), "{msg}");
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
        let py_req = c.dependencies.iter().find(|d| d.name == "Python").unwrap();
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
    fn template_constants_resolve_major_families() {
        // Seeded TEMPLATE_CONSTANTS must not be unknown-name errors.
        let src = r#"
name = 'Foo'
version = '1.2.3'
toolchain = {'name': 'foss', 'version': '2024a'}
sources = [
    SOURCE_TAR_GZ,
    SOURCELOWER_TAR_BZ2,
    GITHUB_SOURCE,
    GITHUB_LOWER_SOURCE,
    PYPI_SOURCE,
    GNU_SOURCE,
]
dependencies = []
"#;
        let r = resolve_easyconfig_str(src).expect("constants resolve");
        assert_eq!(r.name, "Foo");
        assert_eq!(r.version, "1.2.3");
    }

    #[test]
    fn parse_tolerates_if_for_and_junk_after_required_fields() {
        let src = r#"
name = 'TolerantApp'
version = '9.9'
toolchain = {'name': 'foss', 'version': '2025b'}
if True:
    configopts = '--bogus'
for x in [1, 2]:
    pass
configopts = unknown_helper() + ' more'
dependencies = [
    ('OpenMPI', '5.0.3'),
]
builddependencies = [
    ('CMake', '3.29.3'),
]
"#;
        let r = resolve_easyconfig_str(src).expect("tolerant parse");
        assert_eq!(r.name, "TolerantApp");
        assert_eq!(r.version, "9.9");
        assert_eq!(r.toolchain.label(), "foss-2025b");
        assert_eq!(r.dependencies.len(), 1);
        assert_eq!(r.dependencies[0].name, "OpenMPI");
        assert_eq!(r.dependencies[0].version, "5.0.3");
        assert_eq!(r.builddependencies[0].name, "CMake");
    }

    #[test]
    fn parse_tree_skips_broken_files_and_reports_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("good.eb"),
            "name = 'Good'\nversion = '1.0'\ntoolchain = {'name': 'foss', 'version': '2025b'}\ndependencies = []\n",
        )
        .unwrap();
        std::fs::write(root.join("bad.eb"), "this is not a valid easyconfig {{{{\n").unwrap();
        std::fs::write(
            root.join("also_good.eb"),
            "name = 'AlsoGood'\nversion = '2.0'\ntoolchain = SYSTEM\ndependencies = []\n",
        )
        .unwrap();
        let tree = parse_easyconfig_tree(root).expect("tree walk");
        assert_eq!(tree.parsed_count(), 2, "got {:?}", tree.candidates);
        assert_eq!(tree.skip_count(), 1, "got {:?}", tree.skipped);
        assert!(tree.skipped[0].path.ends_with("bad.eb"));
        assert!(tree.candidates.iter().any(|c| c.name == "Good"));
        assert!(tree.candidates.iter().any(|c| c.name == "AlsoGood"));
        assert!(tree.coverage() > 0.6 && tree.coverage() < 1.0);
    }

    fn checksum_recipe(checksums_block: &str) -> ResolvedEasyconfig {
        let src = format!(
            "name = 'App'\nversion = '1.0'\n\
             toolchain = {{'name': 'foss', 'version': '2026.1'}}\n\
             sources = ['app-1.0.tar.gz', 'sub-2.0.tar.gz', 'core-3.0.tar.gz']\n\
             patches = ['App-1.0_fix.patch']\n\
             checksums = {checksums_block}\n\
             dependencies = []\n"
        );
        resolve_easyconfig_str(&src).expect("parse checksum recipe")
    }

    #[test]
    fn checksum_lint_accepts_sources_then_patches_order() {
        let r = checksum_recipe(
            "[{'app-1.0.tar.gz': 'aa11'}, {'sub-2.0.tar.gz': 'bb22'},\n \
             {'core-3.0.tar.gz': 'cc33'}, {'App-1.0_fix.patch': 'dd44'}]",
        );
        assert_eq!(r.sources_count, 3);
        assert_eq!(r.patch_names, vec!["App-1.0_fix.patch"]);
        assert!(checksum_structure_findings(&r).is_empty());
    }

    #[test]
    fn checksum_lint_flags_patch_checksum_in_source_slot() {
        // Counts match, so only positional key matching catches a patch
        // checksum occupying a source slot before EasyBuild starts the build.
        let r = checksum_recipe(
            "[{'app-1.0.tar.gz': 'aa11'}, {'App-1.0_fix.patch': 'dd44'},\n \
             {'sub-2.0.tar.gz': 'bb22'}, {'core-3.0.tar.gz': 'cc33'}]",
        );
        let findings = checksum_structure_findings(&r);
        assert_eq!(findings.len(), 1, "got {findings:?}");
        assert!(findings[0].contains("source slot"), "got {findings:?}");
        // and it fails the packaging gate
        let check = check_recipe_deps(&r, &[]);
        assert!(!check.ok());
        assert!(check.missing.iter().any(|m| m.role == "packaging"));
    }

    #[test]
    fn checksum_lint_flags_count_mismatch_and_allows_multiarch_dicts() {
        // 3 sources + 1 patch but only 2 checksum entries -> count finding.
        let short = checksum_recipe("[{'app-1.0.tar.gz': 'aa11'}, {'App-1.0_fix.patch': 'dd44'}]");
        assert!(checksum_structure_findings(&short)
            .iter()
            .any(|f| f.contains("positionally")));
        // A multi-key (per-arch) dict is ONE list entry, not two values.
        let arch = "name = 'Sdk'\nversion = '1.0'\n\
                    toolchain = SYSTEM\n\
                    sources = ['sdk_%(arch)s.tar.gz']\n\
                    checksums = [{'sdk_aarch64.tar.gz': 'aa', 'sdk_x86_64.tar.gz': 'bb'}]\n\
                    dependencies = []\n";
        let r = resolve_easyconfig_str(arch).expect("parse arch recipe");
        assert_eq!(r.sources_count, 1);
        assert_eq!(r.checksum_entry_keys.len(), 1);
        assert!(checksum_structure_findings(&r).is_empty());
    }

    #[test]
    fn missing_dep_reason_names_nearest_generations() {
        let recipe = resolve_easyconfig_str(
            "name = 'App'\nversion = '1.0'\n\
             toolchain = {'name': 'foss', 'version': '2026.1'}\n\
             dependencies = [('Foo', '1.0')]\n",
        )
        .expect("parse");
        let other = resolve_easyconfig_str(
            "name = 'Foo'\nversion = '0.9'\n\
             toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
             dependencies = []\n",
        )
        .expect("parse")
        .to_candidate();
        let check = check_recipe_deps(&recipe, &[other]);
        assert!(!check.ok());
        let reason = &check.missing[0].reason;
        assert!(
            reason.contains("available at other generations")
                && reason.contains("0.9 @ GCCcore-13.3.0"),
            "got {reason}"
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

    #[test]
    fn merge_candidates_preserves_versionsuffix_variants() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let candidate = |suffix: Option<&str>, path: &str| Candidate {
            name: "PyTorch".into(),
            version: "2.9.1".into(),
            toolchain: tc.clone(),
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: path.into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
        };
        let merged = merge_candidates_with_precedence(&[vec![
            candidate(None, "PyTorch-2.9.1-foss-2024a.eb"),
            candidate(
                Some("-CUDA-12.6.0"),
                "PyTorch-2.9.1-foss-2024a-CUDA-12.6.0.eb",
            ),
        ]]);
        assert_eq!(merged.len(), 2);
        assert!(merged
            .iter()
            .any(|candidate| candidate.versionsuffix.is_none()));
        assert!(merged
            .iter()
            .any(|candidate| { candidate.versionsuffix.as_deref() == Some("-CUDA-12.6.0") }));
    }

    fn blank_recipe() -> ResolvedEasyconfig {
        ResolvedEasyconfig {
            name: "X".into(),
            version: "1.0".into(),
            versionsuffix: None,
            toolchain: Toolchain {
                name: "gfbf".into(),
                version: "2024a".into(),
            },
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            easyconfig_path: "X.eb".into(),
            easyblock: None,
            configopts: None,
            moduleclass: None,
            homepage: None,
            checksums: vec![],
            sources_count: 0,
            patch_names: vec![],
            checksum_entry_keys: vec![],
        }
    }

    #[test]
    fn packaging_gate_requires_moduleclass_checksums_not_easyblock() {
        let mut r = blank_recipe();
        let errs = packaging_gate(&r, &[]).unwrap_err();
        // A missing easyblock is name-derived by EasyBuild, never a gate error.
        assert!(!errs.iter().any(|e| e.contains("easyblock")));
        assert!(errs.iter().any(|e| e.contains("moduleclass")));
        assert!(errs.iter().any(|e| e.contains("checksums")));

        // No explicit easyblock (name-derived) still passes once the genuinely
        // required fields are present — mirrors OpenMPI / nvidia-compilers.
        r.moduleclass = Some("mpi".into());
        r.checksums = vec!["deadbeef".into()];
        assert!(r.easyblock.is_none());
        packaging_gate(&r, &[]).unwrap();

        r.easyblock = Some("MesonNinja".into());
        r.moduleclass = Some("chem".into());
        r.configopts = Some("-Dwith_fortran=true -Dwith_tests=false".into());
        packaging_gate(&r, &["-Dwith_fortran=true", "-Dwith_tests=false"]).unwrap();
        let miss = packaging_gate(&r, &["-Dwith_metatomic=true"]).unwrap_err();
        assert!(miss.iter().any(|e| e.contains("with_metatomic")));
    }

    #[test]
    fn candidate_matches_dep_cross_toolchain() {
        let dep = ResolvedDep {
            name: "quill".into(),
            version: "11.1.0".into(),
            versionsuffix: None,
            toolchain: Some(Toolchain {
                name: "GCCcore".into(),
                version: "13.3.0".into(),
            }),
        };
        let ok = Candidate {
            name: "quill".into(),
            version: "11.1.0".into(),
            toolchain: Toolchain {
                name: "GCCcore".into(),
                version: "13.3.0".into(),
            },
            versionsuffix: None,
            easyconfig_path: "quill.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        let wrong_tc = Candidate {
            toolchain: Toolchain {
                name: "gfbf".into(),
                version: "2024a".into(),
            },
            ..ok.clone()
        };
        assert!(candidate_matches_dep(&ok, &dep));
        assert!(!candidate_matches_dep(&wrong_tc, &dep));
    }

    #[test]
    fn unpinned_dep_requires_hierarchy_member_not_older_gcccore() {
        // CapnProto 1.4.0 only on GCCcore-14.3.0 must NOT satisfy foss-2026.1
        // (GCCcore-15.2.0 hierarchy). Explicit cross-gen pins still match.
        let mut r = blank_recipe();
        r.toolchain = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        r.dependencies = vec![
            ResolvedDep {
                name: "CapnProto".into(),
                version: "1.4.0".into(),
                versionsuffix: None,
                toolchain: None,
            },
            ResolvedDep {
                name: "xtb".into(),
                version: "6.7.1".into(),
                versionsuffix: None,
                toolchain: Some(Toolchain {
                    name: "gfbf".into(),
                    version: "2024a".into(),
                }),
            },
        ];
        let old_capnp = Candidate {
            name: "CapnProto".into(),
            version: "1.4.0".into(),
            toolchain: Toolchain {
                name: "GCCcore".into(),
                version: "14.3.0".into(),
            },
            versionsuffix: None,
            easyconfig_path: "CapnProto-1.4.0-GCCcore-14.3.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        let xtb = Candidate {
            name: "xtb".into(),
            version: "6.7.1".into(),
            toolchain: Toolchain {
                name: "gfbf".into(),
                version: "2024a".into(),
            },
            versionsuffix: None,
            easyconfig_path: "xtb.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        // Minimal foss-2026.1 tree so hierarchy derives with GCCcore-15.2.0.
        let mut foss_def = Candidate {
            name: "foss".into(),
            version: "2026.1".into(),
            toolchain: Toolchain {
                name: "system".into(),
                version: String::new(),
            },
            versionsuffix: None,
            easyconfig_path: "foss-2026.1.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        };
        foss_def.dependencies = vec![
            crate::DepReq {
                name: "GCCcore".into(),
                version_req: "15.2.0".into(),
                versionsuffix: None,
                toolchain: None,
            },
            crate::DepReq {
                name: "GCC".into(),
                version_req: "15.2.0".into(),
                versionsuffix: None,
                toolchain: None,
            },
            crate::DepReq {
                name: "gompi".into(),
                version_req: "2026.1".into(),
                versionsuffix: None,
                toolchain: None,
            },
            crate::DepReq {
                name: "gfbf".into(),
                version_req: "2026.1".into(),
                versionsuffix: None,
                toolchain: None,
            },
        ];
        let universe_old = vec![old_capnp.clone(), xtb.clone(), foss_def.clone()];
        let check_old = check_recipe_deps(&r, &universe_old);
        assert!(
            check_old.missing.iter().any(|m| m.name == "CapnProto"),
            "older GCCcore CapnProto must not satisfy foss-2026.1: {:?}",
            check_old
        );
        assert!(
            check_old.found.iter().any(|f| f.contains("xtb")),
            "explicit cross-gen xtb pin must still match: {:?}",
            check_old.found
        );

        let new_capnp = Candidate {
            toolchain: Toolchain {
                name: "GCCcore".into(),
                version: "15.2.0".into(),
            },
            easyconfig_path: "CapnProto-1.4.0-GCCcore-15.2.0.eb".into(),
            ..old_capnp
        };
        let universe_new = vec![new_capnp, xtb, foss_def];
        let check_new = check_recipe_deps(&r, &universe_new);
        assert!(
            check_new.missing.iter().all(|m| m.name != "CapnProto"),
            "GCCcore-15.2.0 CapnProto must satisfy: {:?}",
            check_new
        );
        assert!(check_new.found.iter().any(|f| f.contains("CapnProto")));
    }

    #[test]
    fn check_recipe_deps_reports_missing_and_found() {
        let mut r = blank_recipe();
        r.dependencies = vec![ResolvedDep {
            name: "Python".into(),
            version: "3.12.3".into(),
            versionsuffix: None,
            toolchain: None,
        }];
        r.builddependencies = vec![ResolvedDep {
            name: "Meson".into(),
            version: "1.4.0".into(),
            versionsuffix: None,
            toolchain: None,
        }];
        let universe = vec![Candidate {
            name: "Python".into(),
            version: "3.12.3".into(),
            toolchain: Toolchain {
                name: "GCCcore".into(),
                version: "13.3.0".into(),
            },
            versionsuffix: None,
            easyconfig_path: "Python.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        }];
        let check = check_recipe_deps(&r, &universe);
        assert!(!check.ok());
        assert!(check.found.iter().any(|f| f.contains("Python")));
        assert!(check
            .missing
            .iter()
            .any(|m| m.name == "Meson" && m.role == "build"));
    }
}
