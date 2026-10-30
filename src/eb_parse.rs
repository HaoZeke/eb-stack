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
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
/// Why an easyconfig could not be read.
pub enum ParseError {
    #[error("IO {0}: {1}")]
    /// The file could not be read.
    ///
    /// The operating-system error is part of the message. It is not also
    /// a source, or `{:#}` prints that text a second time.
    Io(String, std::io::Error),
    #[error("parse {0}: {1}")]
    /// The file could not be understood. Carries the path and the reason,
    /// including a line number where the failure was positional.
    Parse(String, String),
}

/// One resolved dependency entry (2–4 element EasyBuild dependency tuple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDep {
    /// Dependency name.
    pub name: String,
    /// Raw version field after template/local resolution (may be `1.2.3` or `>=1.2`).
    pub version: String,
    #[serde(default)]
    /// Versionsuffix required, when the entry names one.
    pub versionsuffix: Option<String>,
    /// Per-dependency toolchain override (`None` = inherit the easyconfig toolchain).
    #[serde(default)]
    pub toolchain: Option<Toolchain>,
}

/// One `exts_list` entry after resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedExt {
    /// Extension name.
    pub name: String,
    /// Extension version.
    pub version: String,
    /// Patch files the entry's options name (`'patches': [...]`). They sit
    /// beside the easyconfig like its own patches and travel with it.
    #[serde(default)]
    pub patches: Vec<String>,
}

/// Fully resolved easyconfig fields (templates and locals applied).
///
/// Solver-facing co-selection uses [`Self::to_candidate`]. Packaging /
/// contribution checks also use the optional metadata fields below
/// (`easyblock`, `configopts`, `moduleclass`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEasyconfig {
    /// Package name.
    pub name: String,
    /// Package version after template expansion.
    pub version: String,
    #[serde(default)]
    /// Versionsuffix, when the easyconfig sets one.
    pub versionsuffix: Option<String>,
    /// Toolchain this recipe builds against.
    pub toolchain: Toolchain,
    #[serde(default)]
    /// Runtime dependencies.
    pub dependencies: Vec<ResolvedDep>,
    #[serde(default)]
    /// Build-time-only dependencies.
    pub builddependencies: Vec<ResolvedDep>,
    #[serde(default)]
    /// Bundled extensions.
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
    /// Expanded `source_urls` entries, used to classify which artifact the
    /// recipe actually downloads.
    #[serde(default)]
    pub source_urls: Vec<String>,
    /// Resolved `sources` entries as `(filename, download_filename)`: the name
    /// EasyBuild saves the artifact under and the name it asks each
    /// `source_urls` entry for, which differ when the entry is a dict with a
    /// `download_filename`. A `source_urls` key inside the dict replaces the
    /// recipe-level list for that entry, as it does in EasyBuild.
    #[serde(default)]
    pub source_files: Vec<SourceFile>,
    /// Patch file names from `patches` (tuple/dict entries reduced to the name).
    #[serde(default)]
    pub patch_names: Vec<String>,
    /// Per-`checksums`-LIST-ENTRY dict keys (empty inner vec for plain-string
    /// entries). Parallel to the checksums list entries, not the flattened
    /// values, so a multi-arch dict entry stays one entry.
    #[serde(default)]
    pub checksum_entry_keys: Vec<Vec<String>>,
    /// Checksums the recipe states by filename, for the entries written as
    /// `{'name': 'sha…'}`. Position says nothing once one entry carries
    /// several hashes.
    #[serde(default)]
    pub checksums_by_filename: BTreeMap<String, String>,
    /// The recipe sets `components`: it bundles other packages, whatever its
    /// easyblock is called.
    #[serde(default)]
    pub has_components: bool,
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
            easyblock: self.easyblock.clone(),
            moduleclass: self.moduleclass.clone(),
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
///
/// Quote state carries across lines, because a `description` is triple-quoted
/// and routinely runs to several. Doxygen's names C# in one, and treating that
/// `#` as a comment removes the closing `"""` along with the rest of the line,
/// so the toolchain assignment below it lands inside a string that never ends
/// and the recipe reads as having no toolchain at all.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut triple: Option<u8> = None;
    for line in src.lines() {
        let opened_before = triple.is_some();
        let b = line.as_bytes();
        let mut in_s = false;
        let mut in_d = false;
        let mut cut = line.len();
        let mut i = 0usize;
        while i < b.len() {
            let c = b[i];
            let tripled =
                (c == b'"' || c == b'\'') && b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c);
            match triple {
                Some(quote) => {
                    if tripled && c == quote {
                        triple = None;
                        i += 3;
                        continue;
                    }
                }
                None => {
                    if tripled && !in_s && !in_d {
                        triple = Some(c);
                        i += 3;
                        continue;
                    }
                    if c == b'\'' && !in_d {
                        in_s = !in_s;
                    } else if c == b'"' && !in_s {
                        in_d = !in_d;
                    } else if c == b'#' && !in_s && !in_d {
                        cut = i;
                        break;
                    }
                }
            }
            i += 1;
        }
        // Inside a multi-line string the line is content, so it is kept as it
        // stands, blank or not.
        if opened_before {
            out.push_str(line);
            out.push('\n');
            continue;
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
    arch: String,
    /// Statements the tolerant loop could not represent. Collected always;
    /// only `--strict` callers ask for them.
    skipped: Vec<SkippedStatement>,
    /// Open bracket nesting, `[`, `(` and `{`. Python joins adjacent string
    /// literals across lines only inside brackets, and this is what tells the
    /// two cases apart.
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, arch: &str) -> Self {
        let mut env = HashMap::new();
        // Seed full EasyBuild TEMPLATE_CONSTANTS (%(…)s applied later).
        for (name, value) in EB_TEMPLATE_CONSTANTS {
            env.insert((*name).to_string(), Value::Str((*value).to_string()));
        }
        Self {
            src: src.as_bytes(),
            pos: 0,
            env,
            arch: arch.to_string(),
            skipped: Vec::new(),
            depth: 0,
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

    /// 1-based line number of a byte offset.
    fn line_of(&self, offset: usize) -> usize {
        self.src[..offset.min(self.src.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count()
            + 1
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
                // `if ARCH == 'x86_64':` is how a recipe adds a dependency or
                // a source name for one architecture, and the `else` is the
                // name used on every other architecture. Kaleido's wheel and
                // Go's tarball are selected that way. The guard was
                // unmodeled, which dropped the assignment, and a `sources`
                // line that then names the missing variable cannot be planned
                // with.
                if self.take_arch_branch() {
                    continue;
                }
                self.skip_compound_or_line();
                continue;
            }
            let append_at = self.pos;
            match self.try_list_append() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(message) => {
                    self.pos = append_at;
                    let line = self.line_of(append_at);
                    if !self.skip_one_statement() {
                        break;
                    }
                    let text = std::str::from_utf8(&self.src[append_at..self.pos])
                        .unwrap_or("<non-utf8 statement>")
                        .trim()
                        .to_string();
                    self.skipped.push(SkippedStatement {
                        line,
                        message,
                        text,
                    });
                    continue;
                }
            }
            let start = self.pos;
            // A statement that failed mid-bracket leaves the counter raised;
            // each statement starts at the outermost level by definition.
            self.depth = 0;
            match self.parse_assignment() {
                Ok(()) => {}
                Err(message) => {
                    // Tolerance is the default, but the reason is not thrown
                    // away: strict callers report what could not be modelled.
                    self.pos = start;
                    let line = self.line_of(start);
                    if !self.skip_one_statement() {
                        break;
                    }
                    let text = std::str::from_utf8(&self.src[start..self.pos])
                        .unwrap_or("<non-utf8 statement>")
                        .trim()
                        .to_string();
                    self.skipped.push(SkippedStatement {
                        line,
                        message,
                        text,
                    });
                }
            }
        }
        Ok(())
    }

    /// Enter the `if` / `elif` / `else` arm that applies to this architecture.
    ///
    /// Returns false, with the cursor unmoved, when the statement is not an
    /// `ARCH` comparison this parser can decide. The caller then skips the
    /// whole compound, which is what a guard written for something other
    /// than the architecture needs.
    ///
    /// A true arm is left for the main loop to parse. A false arm is
    /// skipped, and so is every later `elif` that is also false. `else`
    /// runs only when no earlier arm matched. A true `if` does not consume
    /// the `else`: the main loop skips that compound on the next iteration,
    /// so the other architecture's assignment is not also applied.
    fn take_arch_branch(&mut self) -> bool {
        let Some(header) = self.peek_header() else {
            return false;
        };
        if !header.starts_with("if ") {
            return false;
        }
        let Some(holds) = arch_header_holds(&header, &self.arch) else {
            return false;
        };
        self.consume_line();
        if holds {
            return true;
        }
        self.skip_indented_body();
        loop {
            self.skip_ws();
            let Some(header) = self.peek_header() else {
                break;
            };
            if header == "else:" {
                self.consume_line();
                return true;
            }
            if header.starts_with("elif ") {
                let Some(holds) = arch_header_holds(&header, &self.arch) else {
                    break;
                };
                self.consume_line();
                if holds {
                    return true;
                }
                self.skip_indented_body();
                continue;
            }
            break;
        }
        true
    }

    /// The current line, trimmed, without consuming it.
    fn peek_header(&self) -> Option<String> {
        let rest = self.src.get(self.pos..)?;
        let end = rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
        let line = std::str::from_utf8(&rest[..end]).ok()?;
        Some(line.trim().to_string())
    }

    /// Move past the current physical line.
    fn consume_line(&mut self) {
        if let Some(rel) = self.src[self.pos..].iter().position(|&c| c == b'\n') {
            self.pos += rel + 1;
        } else {
            self.pos = self.src.len();
        }
    }

    /// `name.append(value)` on a list or tuple already assigned.
    ///
    /// Returns false when the statement is not an append, with the cursor
    /// unmoved. A malformed append is an error so the tolerant loop can
    /// record it and move on.
    fn try_list_append(&mut self) -> Result<bool, String> {
        let saved = self.pos;
        let name = match self.parse_ident() {
            Ok(name) => name,
            Err(_) => {
                self.pos = saved;
                return Ok(false);
            }
        };
        self.skip_ws();
        if self.peek() != Some(b'.') {
            self.pos = saved;
            return Ok(false);
        }
        self.pos += 1;
        self.skip_ws();
        let method = match self.parse_ident() {
            Ok(method) => method,
            Err(_) => {
                self.pos = saved;
                return Ok(false);
            }
        };
        if method != "append" {
            self.pos = saved;
            return Ok(false);
        }
        self.skip_ws();
        if self.bump() != Some(b'(') {
            self.pos = saved;
            return Ok(false);
        }
        let arg = self.parse_expr()?;
        self.skip_ws();
        if self.bump() != Some(b')') {
            return Err(self.err(format!("expected ')' after {name}.append")));
        }
        match self.env.get_mut(&name) {
            Some(Value::List(items) | Value::Tuple(items)) => items.push(arg),
            _ => {
                return Err(self.err(format!("{name}.append, but {name} is not a list or tuple")));
            }
        }
        Ok(true)
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
        self.skip_indented_body();
    }

    /// Skip the indented body that follows a header the caller already consumed.
    fn skip_indented_body(&mut self) {
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
        // `_a, _b = version.split('_')` is how SPM takes the release and the
        // update out of one version string. The names are bound before the
        // source list is built from them.
        if self.peek() == Some(b',') {
            return self.finish_unpack(name);
        }
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
                (Some(Value::List(mut a)), Value::List(b) | Value::Tuple(b)) => {
                    a.extend(b);
                    self.env.insert(name, Value::List(a));
                }
                (Some(Value::Tuple(mut a)), Value::Tuple(b)) => {
                    a.extend(b);
                    self.env.insert(name, Value::Tuple(a));
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

    /// `a, b = sequence`. The sequence has to have one value per name.
    fn finish_unpack(&mut self, first: String) -> Result<(), String> {
        let mut names = vec![first];
        while self.peek() == Some(b',') {
            self.pos += 1;
            self.skip_ws();
            if self.peek() == Some(b'=') {
                break;
            }
            names.push(self.parse_ident()?);
            self.skip_ws();
        }
        if self.bump() != Some(b'=') {
            return Err(self.err("expected '=' in an unpacking assignment"));
        }
        let value = self.parse_expr()?;
        let items = match value {
            Value::List(items) | Value::Tuple(items) => items,
            other => {
                return Err(self.err(format!(
                    "unpacking assignment needs a sequence, got {other:?}"
                )))
            }
        };
        if items.len() != names.len() {
            return Err(self.err(format!(
                "unpacking assignment expected {} value(s), got {}",
                names.len(),
                items.len()
            )));
        }
        for (name, item) in names.into_iter().zip(items) {
            self.env.insert(name, item);
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
        self.slice_str(start).map(str::to_string)
    }

    /// The source between `start` and the cursor, as text.
    ///
    /// The lexer only ever advances over bytes it has classified as ASCII, so
    /// the span is on a character boundary. Reporting rather than unwrapping
    /// keeps a malformed span a parse error with a line number instead of a
    /// panic in a library.
    fn slice_str(&self, start: usize) -> Result<&str, String> {
        let end = self.pos.min(self.src.len());
        let bytes = self.src.get(start..end).ok_or_else(|| {
            self.err(format!(
                "internal: span {start}..{end} is outside the source"
            ))
        })?;
        std::str::from_utf8(bytes)
            .map_err(|e| self.err(format!("span {start}..{end} is not valid UTF-8: {e}")))
    }

    /// One `.method(args)` call on a value already parsed.
    ///
    /// String methods derive one field from another: `zmqversion.split('.')[0]`,
    /// `'.'.join(parts)`, `str.format`, plus case and replacement. A dict's
    /// `.get` is how Go, Anaconda3 and cuTENSOR pick the architecture token
    /// in a tarball name. Anything else is an error, because a method whose
    /// result is guessed would put a wrong version in a field.
    fn parse_method_call(&mut self, receiver: Value) -> Result<Value, String> {
        let name = self.parse_ident()?;
        self.skip_ws();
        if self.bump() != Some(b'(') {
            return Err(self.err(format!("method {name} is not called")));
        }
        self.depth += 1;
        let mut positional = Vec::new();
        let mut keywords = Vec::new();
        let mut saw_keyword = false;
        loop {
            self.skip_ws();
            if self.peek() == Some(b')') {
                self.pos += 1;
                break;
            }
            if let Some((key, value)) = self.parse_keyword_arg()? {
                saw_keyword = true;
                keywords.push((key, value));
            } else if saw_keyword {
                return Err(self.err(format!(
                    "positional argument follows a keyword in .{name}()"
                )));
            } else {
                positional.push(self.parse_expr()?);
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                other => {
                    return Err(self.err(format!(
                        "expected ',' or ')' in a method call, got {:?}",
                        other.map(|c| c as char)
                    )));
                }
            }
        }
        self.depth -= 1;
        call_method(&receiver, &name, &positional, &keywords).map_err(|e| self.err(e))
    }

    /// `name=expr` as one keyword argument, or `None` when the argument is
    /// positional. The cursor is unchanged when it is not a keyword.
    fn parse_keyword_arg(&mut self) -> Result<Option<(String, Value)>, String> {
        let saved = self.pos;
        let name = match self.parse_ident() {
            Ok(name) => name,
            Err(_) => {
                self.pos = saved;
                return Ok(None);
            }
        };
        self.skip_ws();
        let is_keyword = self.peek() == Some(b'=') && self.src.get(self.pos + 1) != Some(&b'=');
        if !is_keyword {
            self.pos = saved;
            return Ok(None);
        }
        self.pos += 1;
        let value = self.parse_expr()?;
        Ok(Some((name, value)))
    }

    /// A primary and any `[...]` subscripts applied to it.
    ///
    /// `('protobuf', version[2:])` and `'%s.%s' % (local_ver, patchlevels[0])`
    /// are how recipes derive one field from another, and a parser that stops
    /// at the bracket loses the whole dependency list.
    fn parse_postfix(&mut self) -> Result<Value, String> {
        // `int(text)` is a call, not a name. EvidentialGene indexes a month
        // table with `int(version.split('.')[1])`.
        let mut val = if self.looking_at_call("int") {
            self.parse_int_call()?
        } else {
            self.parse_primary()?
        };
        loop {
            let before_ws = self.pos;
            self.skip_ws();
            let crossed_line = self.src[before_ws..self.pos].contains(&b'\n');
            // A method call chains the same way a subscript does:
            // `zmqversion.split('.')[0]` is both.
            if self.peek() == Some(b'.') && !(crossed_line && self.depth == 0) {
                self.pos += 1;
                val = self.parse_method_call(val)?;
                continue;
            }
            // Outside brackets a `[` on the next line opens a new statement,
            // not a subscript of this one.
            if self.peek() != Some(b'[') || (crossed_line && self.depth == 0) {
                self.pos = before_ws;
                break;
            }
            self.pos += 1;
            let start = self.parse_subscript_arg()?;
            self.skip_ws();
            let is_slice = self.peek() == Some(b':');
            let stop = if is_slice {
                self.pos += 1;
                self.parse_subscript_arg()?
            } else {
                None
            };
            self.skip_ws();
            if self.peek() != Some(b']') {
                return Err(self.err("subscript is not an index, a key or a slice"));
            }
            self.pos += 1;
            val = subscript(&val, start.as_ref(), stop.as_ref(), is_slice)
                .map_err(|e| self.err(e))?;
        }
        Ok(val)
    }

    /// One subscript argument, or `None` for an omitted slice bound.
    ///
    /// An index, a slice bound, or a dict key: `local_archs[ARCH]` names the
    /// architecture's entry, and a recipe uses that to pick one tarball.
    fn parse_subscript_arg(&mut self) -> Result<Option<Value>, String> {
        self.skip_ws();
        if matches!(self.peek(), Some(b']') | Some(b':')) {
            return Ok(None);
        }
        Ok(Some(self.parse_expr()?))
    }

    fn parse_expr(&mut self) -> Result<Value, String> {
        let mut left = self.parse_postfix()?;
        // String / value binary ops used in real easyconfigs: `+` concat, `%` format.
        loop {
            let before_ws = self.pos;
            self.skip_ws();
            let crossed_line = self.src[before_ws..self.pos].contains(&b'\n');
            // Adjacent string literals are one string in Python, which is how
            // easyconfigs keep a long command under the line limit. An f-string
            // counts: RDKit writes an extract command as parenthesized
            // `f'...' f'...'` pieces. Inside brackets they may span lines;
            // outside them Python needs the literals on one logical line, and
            // joining across a newline there would swallow the next statement.
            let join_strings = matches!(left, Value::Str(_))
                && (self.depth > 0 || !crossed_line)
                && (matches!(self.peek(), Some(b'\'' | b'"')) || self.starts_fstring());
            if join_strings {
                let right = self.parse_postfix()?;
                left = match (left, right) {
                    (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                    (a, b) => {
                        return Err(self.err(format!("unsupported implicit concat: {a:?} {b:?}")));
                    }
                };
                continue;
            }
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let right = self.parse_postfix()?;
                    left = match (left, right) {
                        (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                        (Value::Str(a), Value::Int(b)) => Value::Str(format!("{a}{b}")),
                        (Value::Int(a), Value::Str(b)) => Value::Str(format!("{a}{b}")),
                        // `builddependencies = [...] + local_extra_backends`
                        // is how a recipe adds an optional block, and dropping
                        // it would leave the list half-read.
                        (Value::List(a), Value::List(b)) => {
                            Value::List(a.into_iter().chain(b).collect())
                        }
                        (Value::Tuple(a), Value::Tuple(b)) => {
                            Value::Tuple(a.into_iter().chain(b).collect())
                        }
                        (a, b) => {
                            return Err(self.err(format!("unsupported + operands: {a:?} + {b:?}")));
                        }
                    };
                }
                Some(b'%') => {
                    self.pos += 1;
                    let right = self.parse_postfix()?;
                    left = match (left, right) {
                        (Value::Str(fmt), Value::Str(arg)) => {
                            Value::Str(python_percent_format_one(&fmt, &arg))
                        }
                        (Value::Str(fmt), Value::Int(arg)) => {
                            Value::Str(python_percent_format_one(&fmt, &arg.to_string()))
                        }
                        (Value::Str(fmt), Value::Tuple(args)) => {
                            // Python raises TypeError when the operand count does not
                            // match the conversion count. Silently dropping extra
                            // operands (or leaving a `%s` unfilled) would put a wrong
                            // value in the field instead of skipping it, so refuse.
                            let need = count_percent_conversions(&fmt);
                            if need != args.len() {
                                return Err(self.err(format!(
                                    "% format has {need} conversion(s) but {} operand(s)",
                                    args.len()
                                )));
                            }
                            let mut out = fmt;
                            for arg in &args {
                                let s = match arg {
                                    Value::Str(s) => s.clone(),
                                    Value::Int(i) => i.to_string(),
                                    other => {
                                        return Err(self.err(format!(
                                            "unsupported % tuple operand: {other:?}"
                                        )));
                                    }
                                };
                                out = python_percent_format_one(&out, &s);
                            }
                            Value::Str(out)
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
            b'f' | b'F' if self.starts_fstring() => self.parse_fstring(),
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

    /// Evaluate a fragment of source in this parser's own environment.
    ///
    /// Used for the inside of an f-string field, which is an expression like
    /// any other and deserves the same reader rather than a second, weaker
    /// one.
    fn eval_fragment(&self, fragment: &str) -> Result<Value, String> {
        let mut sub = Parser {
            src: fragment.as_bytes(),
            pos: 0,
            env: self.env.clone(),
            arch: self.arch.clone(),
            skipped: Vec::new(),
            // A fragment sits inside braces, so the bracket-sensitive rules
            // behave as if nested, which is what it is.
            depth: self.depth + 1,
        };
        sub.parse_expr()
    }

    /// Whether what follows is an f-string rather than a name beginning with f.
    fn starts_fstring(&self) -> bool {
        let mut at = self.pos;
        // Accept fr'' and rf'' as well, which a recipe writes for a path.
        while matches!(self.src.get(at), Some(b'f' | b'F' | b'r' | b'R')) && at < self.pos + 2 {
            at += 1;
        }
        at > self.pos && matches!(self.src.get(at), Some(b'\'' | b'"'))
    }

    /// An f-string, with `{name}` filled from what the file has defined.
    ///
    /// 221 recipes use one and 47 put it in a field that decides identity or
    /// dependencies, so a parser that stops at the `f` reads those as having
    /// no version at all. Only a bare name is substituted: a format spec or a
    /// conversion is an error rather than a guess, because a silently
    /// mis-rendered version is worse than a recipe that is skipped.
    fn parse_fstring(&mut self) -> Result<Value, String> {
        while matches!(self.peek(), Some(b'f' | b'F' | b'r' | b'R')) {
            self.pos += 1;
        }
        let Value::Str(raw) = self.parse_string()? else {
            return Err(self.err("f-string body is not a string"));
        };
        let mut out = String::new();
        let mut rest = raw.as_str();
        while let Some(at) = rest.find(['{', '}']) {
            let (before, tail) = rest.split_at(at);
            out.push_str(before);
            let opener = &tail[..1];
            let doubled = tail.get(1..2) == Some(opener);
            if doubled {
                out.push_str(opener);
                rest = &tail[2..];
                continue;
            }
            if opener == "}" {
                return Err(self.err("unmatched } in f-string"));
            }
            let Some(end) = tail.find('}') else {
                return Err(self.err("unterminated { in f-string"));
            };
            let expr = tail[1..end].trim();
            // A conversion or a format spec would change the text, and
            // guessing at one would put a wrong value in a field. A colon
            // inside brackets is a slice, which QuantumESPRESSO uses to
            // take the short hash of a git commit: `{local_hash[:8]}`.
            if field_has_format_spec(expr) {
                return Err(self.err(format!("f-string field {expr} carries a format spec")));
            }
            // Anything else is an ordinary expression, and the parser already
            // reads those: `{local_archs[ARCH]}` is a subscript, which a
            // recipe uses to name one architecture's tarball.
            match self.eval_fragment(expr)? {
                Value::Str(s) => out.push_str(&s),
                Value::Int(i) => out.push_str(&i.to_string()),
                other => {
                    return Err(self.err(format!("f-string field {expr} is {other:?}")));
                }
            }
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        Ok(Value::Str(out))
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
            let s = self.slice_str(start)?;
            return Ok(Value::Str(s.to_string()));
        }
        let s = self.slice_str(start)?;
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
            // The machine's architecture, which a recipe uses bare as well as
            // through `%(arch)s`: IJulia asks for Julia at `-linux-%s' % ARCH`.
            "ARCH" => Ok(Value::Str(self.arch.clone())),
            other => {
                if let Some(v) = self.env.get(other) {
                    Ok(v.clone())
                } else {
                    Err(self.err(format!("unknown name '{other}'")))
                }
            }
        }
    }

    /// `name(` with `name` as a whole identifier, not a prefix of a longer one.
    fn looking_at_call(&self, name: &str) -> bool {
        let mut i = self.pos;
        while matches!(self.src.get(i), Some(c) if c.is_ascii_whitespace()) {
            i += 1;
        }
        let bytes = name.as_bytes();
        if !self.src[i..].starts_with(bytes) {
            return false;
        }
        let after = i + bytes.len();
        if matches!(self.src.get(after), Some(c) if c.is_ascii_alphanumeric() || *c == b'_') {
            return false;
        }
        let mut j = after;
        while matches!(self.src.get(j), Some(c) if c.is_ascii_whitespace()) {
            j += 1;
        }
        self.src.get(j) == Some(&b'(')
    }

    fn parse_int_call(&mut self) -> Result<Value, String> {
        let name = self.parse_ident()?;
        if name != "int" {
            return Err(self.err(format!("expected int(), got {name}")));
        }
        self.skip_ws();
        if self.bump() != Some(b'(') {
            return Err(self.err("expected '(' after int"));
        }
        self.depth += 1;
        let arg = self.parse_expr()?;
        self.skip_ws();
        if self.peek() == Some(b',') {
            return Err(self.err("int() with a base is not supported"));
        }
        if self.bump() != Some(b')') {
            return Err(self.err("expected ')' after int()"));
        }
        self.depth -= 1;
        match arg {
            Value::Int(i) => Ok(Value::Int(i)),
            Value::Str(text) => text
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| self.err(format!("int() cannot read '{text}'"))),
            other => Err(self.err(format!("int() argument is {other:?}"))),
        }
    }

    /// Byte offset of a `for` that starts a comprehension, at bracket depth 0.
    ///
    /// The closer of the list ends the search: a `for` after it belongs to
    /// another statement.
    fn comprehension_for(&self, from: usize) -> Option<usize> {
        let mut i = from;
        let mut depth = 0i32;
        let mut quote: Option<u8> = None;
        let mut escape = false;
        while i < self.src.len() {
            let c = self.src[i];
            if let Some(q) = quote {
                if escape {
                    escape = false;
                } else if c == b'\\' {
                    escape = true;
                } else if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            match c {
                b'\'' | b'"' => {
                    quote = Some(c);
                    i += 1;
                }
                b'(' | b'[' | b'{' => {
                    depth += 1;
                    i += 1;
                }
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                    i += 1;
                }
                _ if depth == 0 && self.keyword_at(i, "for") => return Some(i),
                _ => i += 1,
            }
        }
        None
    }

    fn keyword_at(&self, at: usize, keyword: &str) -> bool {
        let bytes = keyword.as_bytes();
        if !self.src[at..].starts_with(bytes) {
            return false;
        }
        let prev_ok = at == 0 || {
            let prev = self.src[at - 1];
            !prev.is_ascii_alphanumeric() && prev != b'_'
        };
        let next = at + bytes.len();
        let next_ok = self
            .src
            .get(next)
            .is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_');
        prev_ok && next_ok
    }

    /// `[expr for name in seq]`. The element is parsed once per item, after
    /// the name is bound, because it refers to that name.
    fn eval_list_comprehension(&mut self, elem_at: usize, for_at: usize) -> Result<Value, String> {
        let depth = self.depth;
        self.pos = for_at;
        let kw = self.parse_ident()?;
        if kw != "for" {
            return Err(self.err("expected for in a comprehension"));
        }
        let var = self.parse_ident()?;
        self.skip_ws();
        let inn = self.parse_ident()?;
        if inn != "in" {
            return Err(self.err("expected in in a comprehension"));
        }
        let iterable = self.parse_expr()?;
        self.skip_ws();
        if self.keyword_at(self.pos, "if") {
            return Err(self.err("a comprehension filter is not read"));
        }
        if self.bump() != Some(b']') {
            return Err(self.err("a comprehension is not closed by ']'"));
        }
        let end = self.pos;
        let items = match iterable {
            Value::List(items) | Value::Tuple(items) => items,
            other => {
                return Err(self.err(format!("a comprehension iterates {other:?}")));
            }
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let previous = self.env.insert(var.clone(), item);
            self.pos = elem_at;
            self.depth = depth;
            let value = self.parse_expr();
            match previous {
                Some(old) => {
                    self.env.insert(var.clone(), old);
                }
                None => {
                    self.env.remove(&var);
                }
            }
            out.push(value?);
        }
        self.pos = end;
        self.depth = depth - 1;
        Ok(Value::List(out))
    }

    fn parse_list(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'[') {
            return Err(self.err("expected '['"));
        }
        self.depth += 1;
        self.skip_ws();
        if self.peek() != Some(b']') {
            let elem_at = self.pos;
            if let Some(for_at) = self.comprehension_for(elem_at) {
                return self.eval_list_comprehension(elem_at, for_at);
            }
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
        self.depth -= 1;
        Ok(Value::List(items))
    }

    fn parse_tuple_or_group(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'(') {
            return Err(self.err("expected '('"));
        }
        self.depth += 1;
        self.skip_ws();
        if self.peek() == Some(b')') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Tuple(vec![]));
        }
        let first = self.parse_expr()?;
        self.skip_ws();
        if self.peek() == Some(b')') {
            // Single parenthesized expr — treat as bare value (Python group), not 1-tuple,
            // unless a trailing comma was present (handled below).
            self.pos += 1;
            self.depth -= 1;
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
        self.depth -= 1;
        Ok(Value::Tuple(items))
    }

    fn parse_dict(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if self.bump() != Some(b'{') {
            return Err(self.err("expected '{'"));
        }
        self.depth += 1;
        let mut items = Vec::new();
        let mut set_items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                break;
            }
            let key_val = self.parse_expr()?;
            // Braces around comma-separated tuples make a set, and a handful
            // of recipes write their dependency list that way. EasyBuild
            // iterates it and builds, so reading it as a list is what the
            // recipe means; refusing the brace loses the dependencies.
            {
                let mut probe = self.pos;
                while matches!(self.src.get(probe), Some(c) if c.is_ascii_whitespace()) {
                    probe += 1;
                }
                if self.src.get(probe) != Some(&b':') {
                    set_items.push(key_val);
                    self.pos = probe;
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                            continue;
                        }
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        other => {
                            return Err(self.err(format!(
                                "expected ',' or '}}' in set, got {:?}",
                                other.map(|c| c as char)
                            )));
                        }
                    }
                }
            }
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
        self.depth -= 1;
        if items.is_empty() && !set_items.is_empty() {
            return Ok(Value::List(set_items));
        }
        if !set_items.is_empty() {
            return Err(self.err("braces mix set entries with dict entries"));
        }
        Ok(Value::Dict(items))
    }
}

fn unescape_python_str(s: &str) -> String {
    // Triple-quoted bodies are stored raw except common escapes if present.
    s.to_string()
}

/// The string methods a recipe uses to derive one field from another.
fn string_method(receiver: &Value, name: &str, args: &[Value]) -> Result<Value, String> {
    let text = || receiver.clone().expect_str("method receiver");
    match (name, args) {
        ("split", [Value::Str(sep)]) => Ok(Value::List(
            text()?
                .split(sep.as_str())
                .map(|p| Value::Str(p.to_string()))
                .collect(),
        )),
        ("split", []) => Ok(Value::List(
            text()?
                .split_whitespace()
                .map(|p| Value::Str(p.to_string()))
                .collect(),
        )),
        ("join", [Value::List(parts) | Value::Tuple(parts)]) => {
            let sep = text()?;
            let mut out = Vec::with_capacity(parts.len());
            for part in parts {
                out.push(part.clone().expect_str("join operand")?);
            }
            Ok(Value::Str(out.join(&sep)))
        }
        ("replace", [Value::Str(from), Value::Str(to)]) => {
            Ok(Value::Str(text()?.replace(from.as_str(), to)))
        }
        ("lower", []) => Ok(Value::Str(text()?.to_lowercase())),
        ("upper", []) => Ok(Value::Str(text()?.to_uppercase())),
        ("strip", []) => Ok(Value::Str(text()?.trim().to_string())),
        _ => Err(format!(
            "unsupported method .{name}() with {} args",
            args.len()
        )),
    }
}

/// Dispatch a method call. Keyword arguments are only meaningful to
/// `str.format` (`'{local_name}'.format(local_name=local_name)` is how
/// Mamba names its installer). Every other method is positional.
fn call_method(
    receiver: &Value,
    name: &str,
    positional: &[Value],
    keywords: &[(String, Value)],
) -> Result<Value, String> {
    match receiver {
        Value::Dict(entries) => {
            if !keywords.is_empty() {
                return Err(format!("method .{name}() on a dict does not take keywords"));
            }
            dict_method(entries, name, positional)
        }
        Value::Str(text) if name == "format" => {
            python_format(text, positional, keywords).map(Value::Str)
        }
        other => {
            if !keywords.is_empty() {
                return Err(format!("method .{name}() does not take keyword arguments"));
            }
            string_method(other, name, positional)
        }
    }
}

/// `mapping.get(key)` and `mapping.get(key, default)`.
///
/// Go writes `{'aarch64': 'arm64', 'x86_64': 'amd64'}.get(ARCH, ARCH)` so a
/// machine that is not in the dict keeps its own architecture name. A missing
/// key with no default is `None`: formatting that is an error rather than an
/// invented filename.
fn dict_method(entries: &[(String, Value)], name: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        "get" => {
            if args.is_empty() || args.len() > 2 {
                return Err(format!(
                    "dict.get takes a key and an optional default, got {} args",
                    args.len()
                ));
            }
            let key = match &args[0] {
                Value::Str(text) => text,
                other => return Err(format!("dict.get key is not a string: {other:?}")),
            };
            if let Some((_, value)) = entries.iter().find(|(candidate, _)| candidate == key) {
                return Ok(value.clone());
            }
            if args.len() == 2 {
                Ok(args[1].clone())
            } else {
                Ok(Value::None)
            }
        }
        _ => Err(format!("unsupported method .{name}() on a dict")),
    }
}

/// Whether `{field}` carries a conversion or a format spec at bracket depth 0.
///
/// `{local_hash[:8]}` is a slice. `{value:.2f}` and `{value!s}` would change
/// the text, and guessing at either would put a wrong value in a field.
fn field_has_format_spec(field: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in field.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' if depth > 0 => depth -= 1,
            '!' | ':' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

fn is_py_ident(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn format_argument(value: &Value) -> Result<String, String> {
    match value {
        Value::Str(text) => Ok(text.clone()),
        Value::Int(i) => Ok(i.to_string()),
        other => Err(format!("format argument is {other:?}")),
    }
}

/// Python `str.format` for the field shapes easyconfigs use: `{}`, `{0}` and
/// `{name}`. `{{` and `}}` are escapes. A format spec is refused.
fn python_format(
    fmt: &str,
    positional: &[Value],
    keywords: &[(String, Value)],
) -> Result<String, String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::with_capacity(fmt.len());
    let mut i = 0;
    let mut auto = 0usize;
    let mut saw_auto = false;
    let mut saw_index = false;
    while i < chars.len() {
        if chars[i] == '{' {
            if chars.get(i + 1) == Some(&'{') {
                out.push('{');
                i += 2;
                continue;
            }
            let start = i + 1;
            let mut j = start;
            let mut depth = 0i32;
            let mut quote: Option<char> = None;
            let mut closed = false;
            while j < chars.len() {
                let c = chars[j];
                if let Some(q) = quote {
                    if c == q {
                        quote = None;
                    }
                    j += 1;
                    continue;
                }
                match c {
                    '\'' | '"' => quote = Some(c),
                    '[' | '(' | '{' => {
                        depth += 1;
                        j += 1;
                    }
                    ']' | ')' | '}' if depth > 0 => {
                        depth -= 1;
                        j += 1;
                    }
                    '}' => {
                        closed = true;
                        break;
                    }
                    _ => j += 1,
                }
            }
            if !closed {
                return Err("unterminated { in format".into());
            }
            let raw: String = chars[start..j].iter().collect();
            if field_has_format_spec(&raw) {
                return Err(format!("format field {{{raw}}} carries a format spec"));
            }
            let field = raw.trim();
            let value = if field.is_empty() {
                if saw_index {
                    return Err("format mixes automatic and manual field numbering".into());
                }
                saw_auto = true;
                let value = positional.get(auto).ok_or_else(|| {
                    format!(
                        "format field {{}} is past the {} positional argument(s)",
                        positional.len()
                    )
                })?;
                auto += 1;
                value
            } else if field.chars().all(|c| c.is_ascii_digit()) {
                if saw_auto {
                    return Err("format mixes automatic and manual field numbering".into());
                }
                saw_index = true;
                let index: usize = field
                    .parse()
                    .map_err(|_| format!("format index {field} is not a number"))?;
                positional.get(index).ok_or_else(|| {
                    format!(
                        "format field {{{field}}} is past the {} positional argument(s)",
                        positional.len()
                    )
                })?
            } else if is_py_ident(field) {
                keywords
                    .iter()
                    .find(|(key, _)| key == field)
                    .map(|(_, value)| value)
                    .ok_or_else(|| format!("format field {{{field}}} has no keyword argument"))?
            } else {
                return Err(format!(
                    "format field {{{field}}} is not an index or a name"
                ));
            };
            out.push_str(&format_argument(value)?);
            i = j + 1;
            continue;
        }
        if chars[i] == '}' {
            if chars.get(i + 1) == Some(&'}') {
                out.push('}');
                i += 2;
                continue;
            }
            return Err("unmatched } in format".into());
        }
        out.push(chars[i]);
        i += 1;
    }
    Ok(out)
}

/// Python's `x[i]` and `x[a:b]` over a string or a sequence.
///
/// An index outside the value is an error rather than a clamp: the recipe
/// meant a particular element, and inventing one would put a wrong version in
/// a dependency where skipping the recipe is at least visible.
fn subscript(
    val: &Value,
    start: Option<&Value>,
    stop: Option<&Value>,
    is_slice: bool,
) -> Result<Value, String> {
    fn as_index(value: Option<&Value>) -> Result<Option<i64>, String> {
        match value {
            None => Ok(None),
            Some(Value::Int(i)) => Ok(Some(*i)),
            Some(other) => Err(format!("subscript bound is not an integer: {other:?}")),
        }
    }
    // A dict is subscripted by key, and only by key.
    if let Value::Dict(entries) = val {
        if is_slice {
            return Err("a mapping cannot be sliced".into());
        }
        let Some(Value::Str(key)) = start else {
            return Err(format!("mapping key is not a string: {start:?}"));
        };
        return entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("mapping has no key {key}"));
    }
    let start = as_index(start)?;
    let stop = as_index(stop)?;
    fn bound(i: i64, len: usize) -> usize {
        if i < 0 {
            (len as i64 + i).max(0) as usize
        } else {
            (i as usize).min(len)
        }
    }
    fn exact(i: i64, len: usize) -> Result<usize, String> {
        let at = if i < 0 { len as i64 + i } else { i };
        if at < 0 || at as usize >= len {
            return Err(format!("index {i} is outside a value of length {len}"));
        }
        Ok(at as usize)
    }
    match val {
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            if is_slice {
                let a = start.map_or(0, |i| bound(i, chars.len()));
                let b = stop.map_or(chars.len(), |i| bound(i, chars.len()));
                Ok(Value::Str(chars[a.min(b)..b].iter().collect()))
            } else {
                let i = start.ok_or_else(|| "subscript has no index".to_string())?;
                Ok(Value::Str(chars[exact(i, chars.len())?].to_string()))
            }
        }
        Value::List(xs) | Value::Tuple(xs) => {
            if is_slice {
                let a = start.map_or(0, |i| bound(i, xs.len()));
                let b = stop.map_or(xs.len(), |i| bound(i, xs.len()));
                Ok(Value::List(xs[a.min(b)..b].to_vec()))
            } else {
                let i = start.ok_or_else(|| "subscript has no index".to_string())?;
                Ok(xs[exact(i, xs.len())?].clone())
            }
        }
        other => Err(format!("cannot subscript {other:?}")),
    }
}

/// Count `%s` / `%d` conversions, skipping `%%` escapes. Used to reject a
/// tuple `%` whose operand count disagrees with the format string, which
/// Python treats as a TypeError rather than a partial substitution.
fn count_percent_conversions(fmt: &str) -> usize {
    let b = fmt.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i + 1 < b.len() {
        if b[i] == b'%' {
            match b[i + 1] {
                b'%' => {
                    i += 2;
                    continue;
                }
                b's' | b'd' => {
                    n += 1;
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        i += 1;
    }
    n
}

/// Minimal Python-style `%s` / `%d` formatting used in easyconfigs. Replaces
/// the leftmost conversion so tuple args substitute in positional order.
fn python_percent_format_one(fmt: &str, arg: &str) -> String {
    // `%%` is an escaped percent, and collapsing it is not optional: a recipe
    // writes `'-Python-%%(pyver)s-Qt-%s' % local_qtver` precisely so that the
    // EasyBuild template survives this formatting step. Leaving the pair
    // doubled produces a suffix no template expansion will ever match.
    let mut out = String::with_capacity(fmt.len() + arg.len());
    let mut rest = fmt;
    let mut filled = false;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        match rest.as_bytes().get(at + 1) {
            Some(b'%') => {
                out.push('%');
                rest = &rest[at + 2..];
            }
            Some(b's' | b'd') if !filled => {
                out.push_str(arg);
                filled = true;
                rest = &rest[at + 2..];
            }
            _ => {
                out.push('%');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
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

fn host_arch() -> &'static str {
    std::env::consts::ARCH
}

fn arch_spellings(arch: &str) -> Vec<&str> {
    let mut out = vec![arch];
    match arch {
        "aarch64" => out.push("AArch64"),
        "AArch64" => out.push("aarch64"),
        "powerpc64" | "powerpc64le" => out.push("POWER"),
        "POWER" => {
            out.push("powerpc64");
            out.push("powerpc64le");
        }
        _ => {}
    }
    out
}

/// A quoted Python string, single or double quotes, with nothing else in it.
fn unquote_py(text: &str) -> Option<&str> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
    {
        Some(&text[1..text.len() - 1])
    } else {
        None
    }
}

/// Whether `if ARCH == 'x86_64':`, `elif ARCH != "aarch64":` or
/// `if ARCH in ('aarch64', 'riscv64'):` is true for `arch`.
///
/// `None` means the header is not an architecture comparison this parser
/// can decide. Spelling aliases (`aarch64` / `AArch64`, `POWER`) count as
/// the same machine.
fn arch_header_holds(header: &str, arch: &str) -> Option<bool> {
    let header = header.trim();
    let header = header.strip_suffix(':')?.trim();
    let rest = header
        .strip_prefix("if ")
        .or_else(|| header.strip_prefix("elif "))?
        .trim();
    let rest = rest.strip_prefix("ARCH")?;
    let rest = rest.trim_start();
    let (op, rhs) = if let Some(rhs) = rest.strip_prefix("==") {
        ("==", rhs.trim())
    } else if let Some(rhs) = rest.strip_prefix("!=") {
        ("!=", rhs.trim())
    } else {
        let rhs = rest.strip_prefix("in")?;
        let rhs = rhs.trim_start();
        if rhs.starts_with(['(', '[']) {
            ("in", rhs)
        } else {
            return None;
        }
    };
    let spellings = arch_spellings(arch);
    let matches = |name: &str| spellings.contains(&name);
    match op {
        "==" => Some(matches(unquote_py(rhs)?)),
        "!=" => Some(!matches(unquote_py(rhs)?)),
        "in" => {
            let body = rhs
                .strip_prefix('(')
                .and_then(|text| text.strip_suffix(')'))
                .or_else(|| {
                    rhs.strip_prefix('[')
                        .and_then(|text| text.strip_suffix(']'))
                })?;
            let mut hit = false;
            let mut any = false;
            for part in body.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                any = true;
                if matches(unquote_py(part)?) {
                    hit = true;
                }
            }
            any.then_some(hit)
        }
        _ => None,
    }
}

fn build_templates(
    name: &str,
    version: &str,
    versionsuffix: &str,
    tc: &Toolchain,
    arch: &str,
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
    // EasyBuild default.py sets both to `%(namelower)s`. A recipe that assigns
    // github_account, bitbucket_account, or versionprefix replaces these on
    // the final template map; see overlay_config_templates.
    tv.insert("github_account".into(), namelower.clone());
    tv.insert("bitbucket_account".into(), namelower);
    tv.insert("version".into(), version.to_string());
    tv.extend(version_part_templates(version));
    tv.insert("versionsuffix".into(), versionsuffix.to_string());
    tv.insert("toolchain_name".into(), tc.name.clone());
    tv.insert("toolchain_version".into(), tc.version.clone());
    // `%(arch)s` is the machine's own architecture, and a recipe uses it in a
    // versionsuffix: LDC 0.17.6 is carried as `-x86_64` and asked for as
    // `-%(arch)s`, so leaving it unexpanded makes one unreachable from the
    // other.
    tv.insert("arch".into(), arch.to_string());
    // Python's own recipe defines the py* templates from its version, which is
    // right only for Python itself. Everything else gets them from the Python
    // it depends on, and a recipe with no Python dependency has no business
    // resolving `%(pyshortver)s` to its own major.minor.
    let parts: Vec<&str> = version.split('.').collect();
    if name == "Python" && parts.len() >= 2 {
        tv.insert("pyver".into(), version.to_string());
        tv.insert("pymajver".into(), parts[0].to_string());
        tv.insert("pyminver".into(), parts[1].to_string());
        tv.insert("pyshortver".into(), format!("{}.{}", parts[0], parts[1]));
    }
    tv
}

/// Copy forge accounts and `versionprefix` from the recipe onto the template map.
///
/// This is step 3 of EasyBuild's `template_constant_dict`
/// (`TEMPLATE_NAMES_CONFIG`). `build_templates` installs the `default.py`
/// fallback `%(namelower)s` for `github_account` and `bitbucket_account`.
/// Leaving that in place when the recipe names an account makes
/// `GITHUB_SOURCE` download `https://github.com/<namelower>/<name>/archive`
/// instead of the repository the recipe states. The assigned value can itself
/// be a template (`%(namelower)s` is the usual spelling of the default), so it
/// is resolved before it replaces the fallback. An account that is empty or
/// still contains an unresolved `%(...)s` is left alone: a download from the
/// wrong account is worse than the namelower fallback.
///
/// `versionprefix` has an empty framework default, so it is inserted only when
/// the recipe sets it.
fn overlay_config_templates(templates: &mut HashMap<String, String>, env: &HashMap<String, Value>) {
    for key in ["github_account", "bitbucket_account"] {
        let Some(value) = env.get(key) else {
            continue;
        };
        let Ok(raw) = value.expect_str(key) else {
            continue;
        };
        let resolved = apply_templates_str(&raw, templates);
        if resolved.is_empty() || resolved.contains("%(") {
            continue;
        }
        templates.insert(key.to_string(), resolved);
    }
    if let Some(value) = env.get("versionprefix") {
        if let Ok(raw) = value.expect_str("versionprefix") {
            let resolved = apply_templates_str(&raw, templates);
            templates.insert("versionprefix".to_string(), resolved);
        }
    }
}
/// Dependency names that define version templates, and the prefix each uses.
///
/// A recipe writes `versionsuffix = '-Python-%(pyver)s'` and
/// `('Tkinter', '%(pyver)s')`, and the value comes from the Python it depends
/// on rather than from anything the recipe states directly. The tree uses
/// 1456 `%(pyshortver)s`, 656 `%(cudaver)s`, 209 `%(rver)s`, 176
/// `%(javaver)s` and 117 `%(perlver)s`; leaving them unexpanded puts a literal
/// `%(pyver)s` in a module name and turns every such dependency into one that
/// nothing satisfies.
const DEP_VERSION_TEMPLATES: &[(&str, &str)] = &[
    ("Python", "py"),
    ("CUDA", "cuda"),
    ("CUDAcore", "cuda"),
    ("R", "r"),
    ("Java", "java"),
    ("Perl", "perl"),
];

/// The `%(<prefix>ver)s` family, taken from what a recipe depends on.
fn dependency_version_templates(deps: &[&[ResolvedDep]]) -> HashMap<String, String> {
    let mut tv = HashMap::new();
    for dep in deps.iter().flat_map(|list| list.iter()) {
        let Some((_, prefix)) = DEP_VERSION_TEMPLATES
            .iter()
            .find(|(name, _)| *name == dep.name)
        else {
            continue;
        };
        // A version constraint is not a version: `>=3.11` names no single
        // number to substitute, and guessing one would be worse than leaving
        // the template visible.
        if dep.version.is_empty() || !dep.version.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let parts: Vec<&str> = dep.version.split('.').collect();
        tv.insert(format!("{prefix}ver"), dep.version.clone());
        tv.insert(format!("{prefix}majver"), parts[0].to_string());
        if parts.len() > 1 {
            tv.insert(format!("{prefix}minver"), parts[1].to_string());
            tv.insert(
                format!("{prefix}shortver"),
                format!("{}.{}", parts[0], parts[1]),
            );
        }
    }
    tv
}

/// `%(key)s`, compiled once. The pattern is a literal, so a failure here would
/// be a build-time defect rather than anything a recipe can cause.
fn template_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"%\(([^)]+)\)s").expect("template pattern is a compile-time literal")
    })
}

fn apply_templates_str(s: &str, templates: &HashMap<String, String>) -> String {
    // EasyBuild uses %(key)s substitution; iterate for rare nested cases.
    let re = template_pattern();
    let mut cur = s.to_string();
    for _ in 0..8 {
        let mut changed = false;
        let next = re
            .replace_all(&cur, |caps: &regex::Captures| {
                // A capture that did not participate leaves the text alone
                // rather than ending the parse.
                let whole = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
                let Some(key) = caps.get(1).map(|m| m.as_str()) else {
                    return whole.to_string();
                };
                if let Some(v) = templates.get(key) {
                    changed = true;
                    v.clone()
                } else {
                    whole.to_string()
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
        // ('gettext', '0.19.8.1', '', True) asks for the dependency at the
        // system toolchain, which is how that was written before the SYSTEM
        // constant existed. A robot path holds recipes older than the
        // constant, and refusing the boolean drops each one entirely.
        Value::Bool(true) => Ok(Toolchain {
            name: "system".into(),
            version: "system".into(),
        }),
        other => Err(format!("{ctx}: unsupported toolchain value {other:?}")),
    }
}

/// The version an arch-specific dependency has on this machine.
///
/// EasyBuild lets a dependency name one version per architecture, which is how
/// the `Java/1.8` wrapper points at a different JDK on x86_64, POWER and
/// AArch64. Nine recipes upstream use it, and each one that cannot be read
/// takes its whole recipe with it.
fn arch_specific_version(entries: &[(String, Value)], arch: &str) -> Result<String, String> {
    let spellings = arch_spellings(arch);
    let mut fallback = None;
    for (key, value) in entries {
        let Some(arch) = key.strip_prefix("arch=") else {
            return Err(format!("dependency version dict has a non-arch key {key}"));
        };
        if spellings.contains(&arch) {
            return value.expect_str("dep.version");
        }
        if arch == "*" {
            fallback = Some(value);
        }
    }
    match fallback {
        Some(value) => value.expect_str("dep.version"),
        None => Err(format!(
            "no dependency version for {}",
            spellings.first().copied().unwrap_or("ARCH")
        )),
    }
}

fn value_to_dep(val: &Value, arch: &str) -> Result<ResolvedDep, String> {
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
    let version = match &items[1] {
        Value::Dict(entries) => arch_specific_version(entries, arch)?,
        other => other.expect_str("dep.version")?,
    };
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

/// Whether the source assigns `field` at the top level.
///
/// Column zero and an `=` that is not `==`, which is what a top-level
/// easyconfig assignment looks like. Augmented assignment counts: a recipe
/// that appends to a dependency list still has one.
fn assigns_at_top_level(src: &str, field: &str) -> bool {
    src.lines().any(|line| {
        let Some(rest) = line.strip_prefix(field) else {
            return false;
        };
        let rest = rest.trim_start();
        match rest.as_bytes() {
            [b'=', b'=', ..] => false,
            [b'=', ..] => true,
            [b'+', b'=', ..] => true,
            _ => false,
        }
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
            patches: Vec::new(),
        });
    }
    let items = match val {
        Value::Tuple(xs) | Value::List(xs) => xs,
        other => return Err(format!("unsupported exts_list entry: {other:?}")),
    };
    if items.len() < 2 {
        return Err(format!("exts_list entry too short: {items:?}"));
    }
    let patches = items
        .get(2)
        .and_then(|options| match options {
            Value::Dict(kvs) => kvs.iter().find(|(k, _)| k == "patches").map(|(_, v)| v),
            _ => None,
        })
        .and_then(|v| value_list_as_slice(Some(v)).ok())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| match entry {
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
        })
        .unwrap_or_default();
    Ok(ResolvedExt {
        name: items[0].expect_str("ext.name")?,
        version: items[1].expect_str("ext.version")?,
        patches,
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
/// One statement the tolerant parser could not model, and why.
///
/// The parser skips an unrepresentable statement so a single oddity does not
/// cost the whole easyconfig. That is the right default, but it means a recipe
/// can parse "successfully" while a line the author cared about was dropped.
/// These records are what `--strict` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedStatement {
    /// 1-based line the statement starts on.
    pub line: usize,
    /// Why it could not be parsed, already carrying its own line prefix.
    pub message: String,
    /// The statement text as written, trimmed.
    pub text: String,
}

impl SkippedStatement {
    /// The reason with the parser's own ``line N:`` position prefix stripped,
    /// so a caller that prints its own position does not repeat it.
    pub fn reason(&self) -> &str {
        match self.message.split_once(": ") {
            Some((head, rest)) if head.starts_with("line ") => rest,
            _ => self.message.as_str(),
        }
    }
}

impl std::fmt::Display for SkippedStatement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {} ({})", self.line, self.text, self.reason())
    }
}

/// Resolve easyconfig text, also returning the statements the parser skipped.
///
/// [`resolve_easyconfig_str`] is this without the second value; the parse is
/// identical, so asking for the report costs nothing and changes nothing.
pub fn resolve_easyconfig_str_reporting(
    src: &str,
) -> Result<(ResolvedEasyconfig, Vec<SkippedStatement>), ParseError> {
    resolve_easyconfig_str_reporting_for_arch(src, host_arch())
}

/// As [`resolve_easyconfig_str_reporting`], with an explicit ARCH value.
pub fn resolve_easyconfig_str_reporting_for_arch(
    src: &str,
    arch: &str,
) -> Result<(ResolvedEasyconfig, Vec<SkippedStatement>), ParseError> {
    resolve_easyconfig_str_inner(src, arch).map(|(recipe, skipped, _, _)| (recipe, skipped))
}

/// As [`resolve_easyconfig_file`], also returning the skipped statements.
pub fn resolve_easyconfig_file_reporting(
    path: &Path,
) -> Result<(ResolvedEasyconfig, Vec<SkippedStatement>), ParseError> {
    resolve_easyconfig_file_reporting_for_arch(path, host_arch())
}

/// As [`resolve_easyconfig_file_reporting`], with an explicit ARCH value.
pub fn resolve_easyconfig_file_reporting_for_arch(
    path: &Path,
    arch: &str,
) -> Result<(ResolvedEasyconfig, Vec<SkippedStatement>), ParseError> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| ParseError::Io(path.display().to_string(), e))?;
    let (mut resolved, skipped) =
        resolve_easyconfig_str_reporting_for_arch(&raw, arch).map_err(|e| match e {
            ParseError::Parse(_, msg) => ParseError::Parse(path.display().to_string(), msg),
            other => other,
        })?;
    resolved.easyconfig_path = path.display().to_string();
    Ok((resolved, skipped))
}

/// Resolve easyconfig source text to fully expanded fields (no filesystem path).
///
/// Tolerant: a statement the parser cannot model is skipped so the rest of the
/// recipe still resolves. Use [`resolve_easyconfig_str_reporting`] when you
/// need to know what was skipped.
pub fn resolve_easyconfig_str(src: &str) -> Result<ResolvedEasyconfig, ParseError> {
    resolve_easyconfig_str_for_arch(src, host_arch())
}

/// As [`resolve_easyconfig_str`], with an explicit ARCH value.
pub fn resolve_easyconfig_str_for_arch(
    src: &str,
    arch: &str,
) -> Result<ResolvedEasyconfig, ParseError> {
    resolve_easyconfig_str_inner(src, arch).map(|(resolved, _, _, _)| resolved)
}

/// Whose options name a [`LocalFile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalFileOwner {
    /// The easyconfig's own `patches`, `postinstallpatches`, `sources`, or
    /// `checksums`.
    Recipe,
    /// An `exts_list` entry.
    Extension {
        /// Extension name.
        name: String,
        /// Extension version.
        version: String,
    },
    /// A `components` entry of a Bundle.
    Component {
        /// Component name.
        name: String,
        /// Component version.
        version: String,
    },
}

/// Which field named a [`LocalFile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalFileKind {
    /// A `patches` or `postinstallpatches` entry.
    Patch,
    /// A `sources` entry with no download URL and no `git_config`.
    Source,
    /// A patch file named only as a `checksums` key, the way Python's
    /// `patch_ctypes_ld_library_path` is.
    ChecksumPatch,
}

/// A file EasyBuild has to find on disk because nothing downloads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFile {
    /// File name after the owner's templates are applied.
    pub filename: String,
    /// The field that named it.
    pub kind: LocalFileKind,
    /// The recipe, extension, or component whose options name it.
    pub owner: LocalFileOwner,
    /// `alt_location` of a patch dict: EasyBuild searches under this name
    /// instead of the software name, and skips the easyconfig's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_location: Option<String>,
}

/// The recipe and every file it expects EasyBuild to find rather than
/// download: patches of the recipe, its extensions and its components,
/// sources no URL fetches, and patch files named only in `checksums`.
///
/// An extension's or component's options are templated with that entry's
/// own name and version, which is what EasyBuild does, so
/// `'%(name)s-%(version)s_fix.patch'` inside an `exts_list` entry names the
/// extension's file and not the parent's.
pub fn resolve_local_files(src: &str) -> Result<(ResolvedEasyconfig, Vec<LocalFile>), ParseError> {
    resolve_easyconfig_str_inner(src, host_arch()).map(|(recipe, _, _, files)| (recipe, files))
}

/// An extension or bundle component, and the easyblock it names itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedEasyblock {
    /// Extension or component name.
    pub name: String,
    /// `easyblock` in the entry's dict. Absent when the entry names none.
    pub class_name: Option<String>,
}

/// Easyblock choices a recipe makes beyond the top-level `easyblock` line.
///
/// Extensions and bundle components carry their own class, or inherit one from
/// `exts_defaultclass`, `exts_classmap`, or `default_easyblock`. The top-level
/// class stays on [`ResolvedEasyconfig::easyblock`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EasyblockSites {
    /// `exts_defaultclass`, when the easyconfig sets one.
    pub exts_defaultclass: Option<String>,
    /// `default_easyblock` for components that name no class of their own.
    pub default_easyblock: Option<String>,
    /// `exts_classmap`: extension name to easyblock class.
    pub exts_classmap: BTreeMap<String, String>,
    /// One row per `exts_list` entry that parsed.
    pub extensions: Vec<NamedEasyblock>,
    /// One row per `components` entry that parsed.
    pub components: Vec<NamedEasyblock>,
}

/// The recipe plus the easyblock each extension and component resolves to.
pub fn easyblock_sites(src: &str) -> Result<(ResolvedEasyconfig, EasyblockSites), ParseError> {
    easyblock_sites_for_arch(src, host_arch())
}

/// As [`easyblock_sites`], with an explicit ARCH value.
pub fn easyblock_sites_for_arch(
    src: &str,
    arch: &str,
) -> Result<(ResolvedEasyconfig, EasyblockSites), ParseError> {
    resolve_easyconfig_str_inner(src, arch).map(|(recipe, _, sites, _)| (recipe, sites))
}

type InnerResolved = (
    ResolvedEasyconfig,
    Vec<SkippedStatement>,
    EasyblockSites,
    Vec<LocalFile>,
);

fn resolve_easyconfig_str_inner(src: &str, arch: &str) -> Result<InnerResolved, ParseError> {
    let cleaned = strip_comments(src);
    let mut parser = Parser::new(&cleaned, arch);
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
    let mut templates = build_templates(&name, &version, &vs_for_templates, &toolchain, arch);
    // The defaults in build_templates are what EasyBuild uses when the recipe
    // leaves the parameter unset. An assignment has to win, or GITHUB_SOURCE
    // points at the lowercased software name instead of the account the recipe
    // named.
    overlay_config_templates(&mut templates, &parser.env);

    // Apply templates to fields that may contain %(…)s (including nested deps/exts).
    let name = apply_templates_str(&name, &templates);
    let version = apply_templates_str(&version, &templates);
    let versionsuffix = versionsuffix_raw.map(|s| apply_templates_str(&s, &templates));
    // Rebuild templates if name/version changed (rare for name/version themselves).
    let mut templates = build_templates(
        &name,
        &version,
        versionsuffix.as_deref().unwrap_or(""),
        &toolchain,
        arch,
    );
    overlay_config_templates(&mut templates, &parser.env);

    let deps_val = parser
        .env
        .get("dependencies")
        .map(|v| apply_templates_value(v, &templates));
    let build_val = parser
        .env
        .get("builddependencies")
        .map(|v| apply_templates_value(v, &templates));
    // A statement the parser cannot represent is skipped, which is right for
    // an oddity in a field nobody plans with. It is wrong for a dependency
    // list: the field then reads as absent, absent reads as none, and the
    // result is a build order that silently omits what a recipe needs. A
    // recipe that assigns one of these and whose value did not survive parsing
    // cannot be planned with, so it is refused here rather than half-read.
    // `versionsuffix` belongs to the same rule: it is part of the module name,
    // so a suffix that was assigned and not read gives the recipe a different
    // identity than the one it will install under.
    for field in [
        "dependencies",
        "builddependencies",
        "versionsuffix",
        "sources",
    ] {
        if assigns_at_top_level(src, field) && !parser.env.contains_key(field) {
            return Err(ParseError::Parse(
                "<string>".into(),
                format!(
                    "{field} is assigned but could not be read, so planning with this \
                     recipe would get its identity, dependencies, or sources wrong"
                ),
            ));
        }
    }

    let dependencies = value_list_as_slice(deps_val.as_ref())
        .map_err(|e| ParseError::Parse("<string>".into(), e))?
        .iter()
        .map(|entry| value_to_dep(entry, arch))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;
    let builddependencies = value_list_as_slice(build_val.as_ref())
        .map_err(|e| ParseError::Parse("<string>".into(), e))?
        .iter()
        .map(|entry| value_to_dep(entry, arch))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ParseError::Parse("<string>".into(), e))?;
    // What a recipe depends on defines templates the recipe itself uses, so
    // the dependency list has to be read before those can be resolved. Only a
    // second pass, and only when the dependencies define something.
    let dep_tv = dependency_version_templates(&[&dependencies, &builddependencies]);
    let (mut templates, versionsuffix, dependencies, builddependencies) = if dep_tv.is_empty() {
        (templates, versionsuffix, dependencies, builddependencies)
    } else {
        let mut templates = templates;
        templates.extend(dep_tv);
        let versionsuffix = versionsuffix.map(|s| apply_templates_str(&s, &templates));
        let redo = |val: Option<&Value>| -> Result<Vec<ResolvedDep>, ParseError> {
            let applied = val.map(|v| apply_templates_value(v, &templates));
            value_list_as_slice(applied.as_ref())
                .map_err(|e| ParseError::Parse("<string>".into(), e))?
                .iter()
                .map(|entry| value_to_dep(entry, arch))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| ParseError::Parse("<string>".into(), e))
        };
        let dependencies = redo(parser.env.get("dependencies"))?;
        let builddependencies = redo(parser.env.get("builddependencies"))?;
        (templates, versionsuffix, dependencies, builddependencies)
    };
    // After both build_templates calls. Each one installs the namelower
    // fallback, so an earlier override would be discarded. Dependency
    // version templates only extend the map; they do not reset accounts.
    overlay_config_templates(&mut templates, &parser.env);

    // Extensions carry `%(pyver)s` too, so they use the final template set.
    let exts_val = parser
        .env
        .get("exts_list")
        .map(|v| apply_templates_value(v, &templates));
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
    let source_urls = env_string_list(&parser.env, "source_urls", &templates);
    let source_files = source_files_field(&parser.env, &templates);
    let patch_names = patch_names_field(&parser.env, &templates);
    let checksum_entry_keys = checksum_entry_keys_field(&parser.env, &templates);
    let checksums_by_filename = checksums_by_filename_field(&parser.env, &templates);
    let has_components = parser.env.contains_key("components")
        || parser
            .skipped
            .iter()
            .any(|stmt| stmt.text.trim_start().starts_with("components"));
    let local_files = local_files_from(
        src,
        &parser.env,
        &templates,
        &LocalFileContext {
            versionsuffix: versionsuffix.as_deref().unwrap_or(""),
            toolchain: &toolchain,
            arch,
            source_files: &source_files,
            source_urls: &source_urls,
            patch_names: &patch_names,
            checksums_by_filename: &checksums_by_filename,
        },
    );

    let resolved = ResolvedEasyconfig {
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
        source_urls,
        source_files,
        patch_names,
        checksum_entry_keys,
        checksums_by_filename,
        has_components,
    };
    let mut sites = easyblock_sites_from(&parser.env, &templates);
    recover_named_easyblocks(&mut sites, &parser.skipped);
    Ok((resolved, parser.skipped, sites, local_files))
}

struct LocalFileContext<'a> {
    versionsuffix: &'a str,
    toolchain: &'a Toolchain,
    arch: &'a str,
    source_files: &'a [SourceFile],
    source_urls: &'a [String],
    patch_names: &'a [String],
    checksums_by_filename: &'a BTreeMap<String, String>,
}

fn is_url(name: &str) -> bool {
    ["http://", "https://", "ftp://"]
        .iter()
        .any(|scheme| name.starts_with(scheme))
}

/// `patches` entries as EasyBuild's `create_patch_info` reads them: a name,
/// a `(name, level)` or `(name, dest)` pair, or a dict with `name` and
/// optionally `alt_location`. A URL is downloaded, not looked up.
fn patch_specs(value: Option<&Value>) -> Vec<(String, Option<String>)> {
    let Ok(items) = value_list_as_slice(value) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Str(s) => Some((s.clone(), None)),
            Value::Tuple(xs) | Value::List(xs) => xs
                .first()
                .and_then(|x| x.as_str())
                .map(|s| (s.to_string(), None)),
            Value::Dict(kvs) => {
                let get = |key: &str| {
                    kvs.iter()
                        .find(|(k, _)| k == key)
                        .and_then(|(_, v)| v.as_str())
                        .map(str::to_string)
                };
                get("name")
                    .or_else(|| get("filename"))
                    .map(|name| (name, get("alt_location")))
            }
            _ => None,
        })
        .filter(|(name, _)| !name.is_empty() && !is_url(name))
        .collect()
}

fn dict_get<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    let Value::Dict(kvs) = value else {
        return None;
    };
    kvs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The parent's templates with `name`, `version`, and the names derived from
/// them taken from an extension or component instead.
fn entry_templates(
    parent: &HashMap<String, String>,
    name: &str,
    version: &str,
    ctx: &LocalFileContext<'_>,
) -> HashMap<String, String> {
    let mut templates = parent.clone();
    templates.extend(build_templates(
        name,
        version,
        ctx.versionsuffix,
        ctx.toolchain,
        ctx.arch,
    ));
    templates
}

/// `(name, version, raw options)` of each `exts_list` or `components` entry.
/// Name and version resolve with the parent's templates; the options stay raw
/// so the caller can apply the entry's own.
fn sub_entries(
    raw: Option<&Value>,
    templates: &HashMap<String, String>,
) -> Vec<(String, String, Option<Value>)> {
    let Ok(items) = value_list_as_slice(raw) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Tuple(xs) | Value::List(xs) if xs.len() >= 2 => {
                let name = apply_templates_str(xs[0].as_str()?, templates);
                let version = apply_templates_str(&xs[1].expect_str("version").ok()?, templates);
                Some((name, version, xs.get(2).cloned()))
            }
            _ => None,
        })
        .collect()
}

fn local_files_from(
    src: &str,
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
    ctx: &LocalFileContext<'_>,
) -> Vec<LocalFile> {
    let mut out: Vec<LocalFile> = Vec::new();
    let mut push = |filename: String,
                    kind: LocalFileKind,
                    owner: LocalFileOwner,
                    alt_location: Option<String>| {
        let file = LocalFile {
            filename,
            kind,
            owner,
            alt_location,
        };
        if !out.contains(&file) {
            out.push(file);
        }
    };

    for key in ["patches", "postinstallpatches"] {
        let value = env.get(key).map(|v| apply_templates_value(v, templates));
        for (name, alt) in patch_specs(value.as_ref()) {
            push(name, LocalFileKind::Patch, LocalFileOwner::Recipe, alt);
        }
    }

    // A recipe whose `source_urls` statement did not parse still has URLs.
    let recipe_has_urls = !ctx.source_urls.is_empty() || assigns_at_top_level(src, "source_urls");
    for source in ctx.source_files {
        if recipe_has_urls || !source.source_urls.is_empty() || source.git {
            continue;
        }
        if source.filename.is_empty() || is_url(&source.filename) {
            continue;
        }
        push(
            source.filename.clone(),
            LocalFileKind::Source,
            LocalFileOwner::Recipe,
            None,
        );
    }

    for (name, version, options) in sub_entries(env.get("exts_list"), templates) {
        let Some(options) = options else { continue };
        let own = entry_templates(templates, &name, &version, ctx);
        let options = apply_templates_value(&options, &own);
        for key in ["patches", "post_install_patches", "postinstallpatches"] {
            for (file, alt) in patch_specs(dict_get(&options, key)) {
                push(
                    file,
                    LocalFileKind::Patch,
                    LocalFileOwner::Extension {
                        name: name.clone(),
                        version: version.clone(),
                    },
                    alt,
                );
            }
        }
    }

    let mut component_patches: Vec<String> = Vec::new();
    for (name, version, options) in sub_entries(env.get("components"), templates) {
        let Some(options) = options else { continue };
        let own = entry_templates(templates, &name, &version, ctx);
        let options = apply_templates_value(&options, &own);
        for key in ["patches", "postinstallpatches"] {
            for (file, alt) in patch_specs(dict_get(&options, key)) {
                component_patches.push(file.clone());
                push(
                    file,
                    LocalFileKind::Patch,
                    LocalFileOwner::Component {
                        name: name.clone(),
                        version: version.clone(),
                    },
                    alt,
                );
            }
        }
    }

    for key in ctx.checksums_by_filename.keys() {
        let filename = apply_templates_str(key, templates);
        let is_patch = filename.ends_with(".patch") || filename.ends_with(".diff");
        let listed = ctx.patch_names.contains(&filename)
            || component_patches.contains(&filename)
            || ctx.source_files.iter().any(|s| s.filename == filename);
        if !is_patch || listed || is_url(&filename) {
            continue;
        }
        push(
            filename,
            LocalFileKind::ChecksumPatch,
            LocalFileOwner::Recipe,
            None,
        );
    }
    out
}

/// A `components` or `exts_list` statement the parser skipped can still name
/// an easyblock. GFE builds that list as a comprehension plus a tuple whose
/// `preconfigopts` is an f-string join, which is not a value we model, and
/// the `easyblock` key would disappear with the statement.
fn recover_named_easyblocks(sites: &mut EasyblockSites, skipped: &[SkippedStatement]) {
    for stmt in skipped {
        let text = stmt.text.trim_start();
        let into_components = text.starts_with("components");
        let into_extensions = text.starts_with("exts_list");
        if !into_components && !into_extensions {
            continue;
        }
        for class_name in easyblock_dict_literals(text) {
            let dest = if into_components {
                &mut sites.components
            } else {
                &mut sites.extensions
            };
            if dest
                .iter()
                .any(|row| row.class_name.as_deref() == Some(class_name.as_str()))
            {
                continue;
            }
            dest.push(NamedEasyblock {
                name: class_name.clone(),
                class_name: Some(class_name),
            });
        }
    }
}

/// `'easyblock': 'CMakeMake'` and the double-quoted form, in source order.
fn easyblock_dict_literals(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 12 < bytes.len() {
        let quote = bytes[i];
        if quote != b'\'' && quote != b'"' {
            i += 1;
            continue;
        }
        let rest = &text[i + 1..];
        let Some(after_key) = rest.strip_prefix("easyblock") else {
            i += 1;
            continue;
        };
        let after_key = after_key.as_bytes();
        if after_key.first().copied() != Some(quote) {
            i += 1;
            continue;
        }
        let mut j = i + 1 + "easyblock".len() + 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b':' {
            i += 1;
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || (bytes[j] != b'\'' && bytes[j] != b'"') {
            i += 1;
            continue;
        }
        let value_quote = bytes[j];
        j += 1;
        let start = j;
        while j < bytes.len() && bytes[j] != value_quote {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        if let Ok(class_name) = std::str::from_utf8(&bytes[start..j]) {
            if !class_name.is_empty()
                && class_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                out.push(class_name.to_string());
            }
        }
        i = j + 1;
    }
    out
}

fn easyblock_sites_from(
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
) -> EasyblockSites {
    let exts = env
        .get("exts_list")
        .map(|value| apply_templates_value(value, templates));
    let components = env
        .get("components")
        .map(|value| apply_templates_value(value, templates));
    EasyblockSites {
        exts_defaultclass: opt_str_field(env, "exts_defaultclass", templates),
        default_easyblock: opt_str_field(env, "default_easyblock", templates),
        exts_classmap: string_map_field(env, "exts_classmap", templates),
        extensions: named_easyblocks(exts.as_ref()),
        components: named_easyblocks(components.as_ref()),
    }
}

fn named_easyblocks(value: Option<&Value>) -> Vec<NamedEasyblock> {
    let Ok(items) = value_list_as_slice(value) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Str(name) if !name.is_empty() => Some(NamedEasyblock {
                name: name.clone(),
                class_name: None,
            }),
            Value::Tuple(fields) | Value::List(fields) if !fields.is_empty() => {
                let name = fields[0].as_str()?.to_string();
                if name.is_empty() {
                    return None;
                }
                Some(NamedEasyblock {
                    name,
                    class_name: fields.get(2).and_then(dict_easyblock),
                })
            }
            _ => None,
        })
        .collect()
}

fn dict_easyblock(value: &Value) -> Option<String> {
    let Value::Dict(entries) = value else {
        return None;
    };
    entries
        .iter()
        .find(|(key, _)| key == "easyblock")
        .and_then(|(_, class)| class.as_str().map(str::to_string))
}

fn string_map_field(
    env: &HashMap<String, Value>,
    key: &str,
    templates: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let Some(raw) = env.get(key) else {
        return BTreeMap::new();
    };
    let Value::Dict(entries) = apply_templates_value(raw, templates) else {
        return BTreeMap::new();
    };
    entries
        .into_iter()
        .filter_map(|(name, class)| class.as_str().map(|class| (name, class.to_string())))
        .collect()
}

/// Length of a list-valued field after template expansion (0 when absent or
/// not a list).
/// Plain-string entries of a list field, templates expanded. Non-string
/// entries are skipped rather than stringified, since a URL is the only shape
/// worth classifying.
fn env_string_list(
    env: &HashMap<String, Value>,
    key: &str,
    templates: &HashMap<String, String>,
) -> Vec<String> {
    let Some(v) = env.get(key) else {
        return Vec::new();
    };
    let v = apply_templates_value(v, templates);
    let Ok(items) = value_list_as_slice(Some(&v)) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

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

/// One `sources` entry, resolved against the recipe's templates.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceFile {
    /// Name the artifact is saved under, which `checksums` is keyed by.
    pub filename: String,
    /// Name requested from each source URL; equals `filename` unless the
    /// entry sets `download_filename`.
    pub download_filename: String,
    /// Per-entry `source_urls`, when the dict entry carries its own.
    pub source_urls: Vec<String>,
    /// The entry fetches a git checkout (`git_config`), so no download URL
    /// names it and no tarball digest can be computed ahead of the build.
    pub git: bool,
}

/// `sources` as EasyBuild reads it: a plain filename, or a dict with
/// `filename` and optionally `download_filename`, `source_urls`, `git_config`.
fn source_files_field(
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
) -> Vec<SourceFile> {
    let Some(v) = env.get("sources") else {
        return Vec::new();
    };
    let v = apply_templates_value(v, templates);
    let Ok(items) = value_list_as_slice(Some(&v)) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Str(s) => Some(SourceFile {
                filename: s.clone(),
                download_filename: s.clone(),
                ..SourceFile::default()
            }),
            Value::Dict(kvs) => {
                let get = |key: &str| {
                    kvs.iter()
                        .find(|(k, _)| k == key)
                        .and_then(|(_, val)| val.as_str())
                        .map(str::to_string)
                };
                let filename = get("filename")?;
                let urls = kvs
                    .iter()
                    .find(|(k, _)| k == "source_urls")
                    .and_then(|(_, val)| value_list_as_slice(Some(val)).ok())
                    .map(|xs| {
                        xs.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(SourceFile {
                    download_filename: get("download_filename").unwrap_or_else(|| filename.clone()),
                    filename,
                    source_urls: urls,
                    git: kvs.iter().any(|(k, _)| k == "git_config"),
                })
            }
            _ => None,
        })
        .collect()
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
/// Checksums a recipe states by filename rather than by position.
///
/// EasyBuild lets a checksum entry be `{'file.tar.gz': 'sha…'}`, and a site
/// uses that to give one entry several architectures' hashes. Positional
/// indexing then goes wrong for everything after it, so a patch listed last
/// reads as having no checksum at all. Keyed entries are recorded by name so
/// the position never has to be guessed.
fn checksums_by_filename_field(
    env: &HashMap<String, Value>,
    templates: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(v) = env.get("checksums") else {
        return out;
    };
    let v = apply_templates_value(v, templates);
    let Ok(items) = value_list_as_slice(Some(&v)) else {
        return out;
    };
    for item in items {
        let Value::Dict(entries) = item else {
            continue;
        };
        for (filename, value) in entries {
            // A tuple of hashes means any of them is acceptable; the first
            // answers the question "does this artifact have a checksum".
            if let Some(checksum) = checksum_strings_from_value(value).first() {
                out.insert(filename.clone(), checksum.clone());
            }
        }
    }
    out
}

fn checksum_strings_from_value(v: &Value) -> Vec<String> {
    match v {
        Value::Str(s) => vec![s.clone()],
        Value::Dict(items) => items
            .iter()
            .filter_map(|(_, val)| val.expect_str("checksum").ok())
            .collect(),
        // A tuple of hashes is one artifact with alternatives, not several
        // artifacts: OpenMolcas lists two acceptable hashes for its tarball
        // and its patch's checksum comes after. Dropping the entry moved every
        // position after it, so the patch read as having no checksum at all.
        Value::Tuple(items) | Value::List(items) => items
            .iter()
            .find_map(|item| item.expect_str("checksum").ok())
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolve one `.eb` file to fully expanded fields.
pub fn resolve_easyconfig_file(path: &Path) -> Result<ResolvedEasyconfig, ParseError> {
    resolve_easyconfig_file_for_arch(path, host_arch())
}

/// As [`resolve_easyconfig_file`], with an explicit ARCH value.
pub fn resolve_easyconfig_file_for_arch(
    path: &Path,
    arch: &str,
) -> Result<ResolvedEasyconfig, ParseError> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| ParseError::Io(path.display().to_string(), e))?;
    let mut resolved = resolve_easyconfig_str_for_arch(&raw, arch).map_err(|e| match e {
        ParseError::Parse(_, msg) => ParseError::Parse(path.display().to_string(), msg),
        other => other,
    })?;
    resolved.easyconfig_path = path.display().to_string();
    Ok(resolved)
}

/// Parse one `.eb` file into a solver-facing [`Candidate`].
pub fn parse_easyconfig_file(path: &Path) -> Result<Candidate, ParseError> {
    parse_easyconfig_file_for_arch(path, host_arch())
}

/// As [`parse_easyconfig_file`], with an explicit ARCH value.
pub fn parse_easyconfig_file_for_arch(path: &Path, arch: &str) -> Result<Candidate, ParseError> {
    Ok(resolve_easyconfig_file_for_arch(path, arch)?.to_candidate())
}

/// One easyconfig path that could not be parsed into a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEasyconfig {
    /// Easyconfig that was skipped.
    pub path: String,
    /// Why it could not be parsed.
    pub error: String,
}

/// Result of walking an easyconfig tree: successes + skipped unparseable files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParseTreeResult {
    /// Everything that parsed.
    pub candidates: Vec<Candidate>,
    /// Everything that did not, kept so a tree is never silently partial.
    pub skipped: Vec<SkippedEasyconfig>,
}

impl ParseTreeResult {
    /// How many easyconfigs failed to parse.
    pub fn skip_count(&self) -> usize {
        self.skipped.len()
    }

    /// How many parsed successfully.
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
    parse_easyconfig_tree_for_arch(root, host_arch())
}

/// As [`parse_easyconfig_tree`], with an explicit ARCH value.
pub fn parse_easyconfig_tree_for_arch(
    root: &Path,
    arch: &str,
) -> Result<ParseTreeResult, ParseError> {
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
                match parse_easyconfig_file_for_arch(&p, arch) {
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
    /// Dependency that could not be matched.
    pub name: String,
    /// Version the recipe asked for.
    pub version: String,
    /// Versionsuffix required, when one was.
    pub versionsuffix: Option<String>,
    /// Toolchain required, when the entry pinned one.
    pub toolchain: Option<Toolchain>,
    /// Runtime vs build-time role in the recipe.
    pub role: String,
    /// Why nothing matched: absent entirely, or present at the wrong
    /// version or toolchain.
    pub reason: String,
}

/// Result of checking that a recipe's deps exist somewhere in a robot universe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeDepCheck {
    /// Easyconfig that was checked.
    pub recipe: String,
    /// Package it declares.
    pub name: String,
    /// Version it declares.
    pub version: String,
    /// Toolchain it targets.
    pub toolchain: Toolchain,
    /// Easyblock it uses, when it sets one.
    pub easyblock: Option<String>,
    /// Configure flags, after template expansion.
    pub configopts: Option<String>,
    /// Moduleclass, when it sets one.
    pub moduleclass: Option<String>,
    /// How many checksum entries it carries.
    pub checksum_count: usize,
    /// Dependencies the robot tree does not provide.
    pub missing: Vec<MissingDep>,
    /// Dependencies that were matched.
    pub found: Vec<String>,
    /// Set when the recipe's toolchain generation could not be resolved, so
    /// matching ran without a hierarchy to filter by.
    ///
    /// Every entry in `found` is then a name-and-version match that ignores
    /// toolchain, which can pair a recipe with a dependency from an unrelated
    /// generation (the `EB_MAINT_CROSS_GEN` shape). `missing` staying empty is
    /// not evidence the recipe resolves in EasyBuild; callers must say so
    /// rather than report a clean result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unverified_toolchain: Option<String>,
}

impl RecipeDepCheck {
    /// Whether every dependency was found.
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }

    /// Whether the matches carry toolchain evidence, i.e. a hierarchy was known.
    ///
    /// Distinct from [`Self::ok`] on purpose: `ok` answers "was anything left
    /// unmatched", this answers "is a match worth anything".
    pub fn toolchain_verified(&self) -> bool {
        self.unverified_toolchain.is_none()
    }
}

/// Whether a universe candidate satisfies a resolved dep (name + version +
/// optional versionsuffix + optional per-dep toolchain). Cross-toolchain
/// dependencies crossing from a parent toolchain to a core toolchain are first-class here —
/// unlike [`filter_toolchain`] which keeps only the policy toolchain.
///
/// When `dep.toolchain` is **None**, any toolchain with a matching
/// name/version/suffix is accepted (legacy identity match). Prefer
/// [`candidate_matches_dep_for_recipe`] so implicit deps follow EasyBuild's
/// minimal-toolchain search within the recipe generation.
pub fn candidate_matches_dep(c: &Candidate, dep: &ResolvedDep) -> bool {
    candidate_matches_dep_core(c, dep, /*require_hierarchy*/ None)
}

/// Like [`candidate_matches_dep`], but when the dep has **no** explicit
/// toolchain pin, only non-SYSTEM candidates in `hierarchy` count for a
/// non-SYSTEM recipe. EasyBuild may minimize an implicit dependency to a
/// generation member such as GCCcore, but it only selects SYSTEM when the
/// dependency tuple says so explicitly. Explicit fourth-tuple pins still
/// match exactly, including explicitly selected cross-generation dependencies.
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
    // Implicit dependencies minimize within the generation, but do not fall
    // through to SYSTEM unless the recipe itself uses SYSTEM.
    if let Some(h) = hierarchy {
        return h.contains(&c.toolchain)
            && (crate::hierarchy::is_system_toolchain(&h.parent)
                || !crate::hierarchy::is_system_toolchain(&c.toolchain));
    }
    true
}

/// Check that every runtime/build dep of `recipe` appears as a candidate in
/// `universe` (any tree layer already merged). Does **not** run the SAT
/// solver — this is the packaging/robot completeness gate used before `eb`.
///
/// Implicit deps must match a non-SYSTEM hierarchy member of the recipe
/// toolchain (derived from the robot universe when possible), so neither an
/// older-generation candidate nor a SYSTEM fallback can false-pass.
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
                        let labels = if d.toolchain.is_none()
                            && !crate::hierarchy::is_system_toolchain(&h.parent)
                        {
                            h.bare_tuple_labels()
                        } else {
                            h.member_labels()
                        };
                        if labels.is_empty() {
                            return String::new();
                        }
                        format!(
                            " (need hierarchy member of {}-{}: {})",
                            recipe.toolchain.name,
                            recipe.toolchain.version,
                            labels.join(" < ")
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
        unverified_toolchain: hierarchy.is_none().then(|| {
            format!(
                "no known or derivable hierarchy for {}; dependency matches ignore \
                 toolchain and may cross generations",
                recipe.toolchain.label()
            )
        }),
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
        let extension_bundle = matches!(
            recipe.easyblock.as_deref(),
            Some("PythonBundle" | "RPackage" | "Bundle")
        ) && !recipe.exts_list.is_empty();
        if !extension_bundle {
            errs.push("missing checksums".into());
        }
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
    parse_easyconfig_trees_for_arch(roots, host_arch())
}

/// As [`parse_easyconfig_trees`], with an explicit ARCH value.
pub fn parse_easyconfig_trees_for_arch(
    roots: &[&Path],
    arch: &str,
) -> Result<ParseTreeResult, ParseError> {
    let mut acc = ParseTreeResult::default();
    let mut layers: Vec<Vec<Candidate>> = Vec::with_capacity(roots.len());
    for root in roots {
        let r = parse_easyconfig_tree_for_arch(root, arch)?;
        layers.push(r.candidates);
        acc.skipped.extend(r.skipped);
    }
    acc.candidates = merge_candidates_with_precedence(&layers);
    acc.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(acc)
}

/// Candidates built against exactly this toolchain.
/// Keep the policy toolchain and everything below it in its hierarchy.
///
/// A stack is not built at one toolchain. A recipe at `GCC-15.2.0` takes its
/// CMake from `GCCcore-15.2.0` and its licence file from SYSTEM, exactly as
/// EasyBuild's minimal-toolchain search does, so a universe filtered to the
/// policy toolchain alone cannot satisfy the dependencies of its own members
/// and reports them as missing packages.
///
/// `members` is the hierarchy for the policy toolchain, lowest first. Passing
/// an empty slice reduces this to [`filter_toolchain`].
pub fn filter_toolchain_hierarchy(
    cands: &[Candidate],
    tc: &Toolchain,
    members: &[Toolchain],
) -> Vec<Candidate> {
    cands
        .iter()
        .filter(|c| {
            // toolchains_match normalizes SYSTEM, which an easyconfig writes
            // as name and version both "system" while a hierarchy carries it
            // with an empty version. Comparing the fields directly drops every
            // SYSTEM candidate, and the explicit SYSTEM pins that bootstrap a
            // generation (binutils under zlib, for one) then read as missing.
            let same =
                |t: &crate::domain::Toolchain| crate::hierarchy::toolchains_match(&c.toolchain, t);
            same(tc) || members.iter().any(same)
        })
        .cloned()
        .collect()
}

/// Keep only candidates built at exactly this toolchain.
///
/// The narrow form, for callers that mean one level and nothing under it.
/// A solve wants [`filter_toolchain_hierarchy`] instead, since a recipe's own
/// build dependencies routinely sit one level down.
pub fn filter_toolchain(cands: &[Candidate], tc: &Toolchain) -> Vec<Candidate> {
    cands
        .iter()
        .filter(|c| c.toolchain.name == tc.name && c.toolchain.version == tc.version)
        .cloned()
        .collect()
}

/// Which versions of one package a universe already carries at one
/// toolchain generation.
///
/// Struct-shaped like the other request types in this crate so the three
/// fields cannot be transposed at a call site.
#[derive(Debug, Clone, Copy)]
pub struct ExistingVersionsQuery<'a> {
    /// Universe from [`parse_easyconfig_tree`] or [`parse_easyconfig_trees`],
    /// live checkout or historical merge-base tree alike.
    pub universe: &'a [Candidate],
    /// Package name to match, exactly.
    pub name: &'a str,
    /// Toolchain generation to match, name and version.
    pub generation: &'a Toolchain,
}

/// Versions of `query.name` already present at `query.generation`, sorted
/// newest first by the same comparison the miner uses.
///
/// This is the producer for [`crate::miner::is_backfill`]'s
/// `existing_versions`: without it a caller has to hand-build that list,
/// which is why backfill detection had no production call site.
pub fn existing_versions(query: &ExistingVersionsQuery<'_>) -> Vec<String> {
    let mut versions: Vec<String> = query
        .universe
        .iter()
        .filter(|c| {
            c.name == query.name
                && c.toolchain.name == query.generation.name
                && c.toolchain.version == query.generation.version
        })
        .map(|c| c.version.clone())
        .collect();
    versions.sort_by(|a, b| crate::version::cmp_version(b, a));
    versions.dedup();
    versions
}

/// Build a lock from an already-selected candidate set.
///
/// The caller has done the choosing; this records it.
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
            criteria: Vec::new(),
        },
    }
}

/// Check that every dependency of every locked package is itself locked.
///
/// A lock that omits a transitive dependency installs a stack that cannot
/// build, so this is worth checking before one is trusted.
pub fn validate_lock_deps(lock: &StackLock, cands: &[Candidate]) -> Result<(), String> {
    use std::collections::HashMap as Map;
    // A generation carries some packages at more than one toolchain level, and
    // they are different modules: keying by name alone would let the second
    // overwrite the first and then report the survivor as the wrong version.
    let mut by_name: Map<&str, Vec<&LockPackage>> = Map::new();
    for p in &lock.packages {
        by_name.entry(p.name.as_str()).or_default().push(p);
    }
    for p in &lock.packages {
        // The versionsuffix is part of the identity: ant 1.10.15 exists as
        // -Java-17 and -Java-21, and checking the wrong one reports a
        // dependency the lock never needed.
        let Some(c) = cands.iter().find(|c| {
            c.name == p.name
                && c.version == p.version
                && c.versionsuffix == p.versionsuffix
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
                let Some(locked) = by_name.get(d.name.as_str()) else {
                    return Err(format!(
                        "{}={} missing co-selected {role} {}",
                        p.name, p.version, d.name
                    ));
                };
                // A dependency that names a toolchain means that build of the
                // package and no other; one that names none is satisfied by
                // whichever level the stack carries.
                let eligible: Vec<&&LockPackage> = locked
                    .iter()
                    .filter(|candidate| match d.toolchain.as_ref() {
                        Some(want) => {
                            crate::hierarchy::toolchains_match(&candidate.toolchain, want)
                        }
                        None => true,
                    })
                    .collect();
                if eligible.is_empty() {
                    return Err(format!(
                        "{}={} requires {role} {} at {}, which the stack does not carry",
                        p.name,
                        p.version,
                        d.name,
                        d.toolchain
                            .as_ref()
                            .map(|t| format!("{}-{}", t.name, t.version))
                            .unwrap_or_else(|| "any toolchain".into())
                    ));
                }
                if !eligible
                    .iter()
                    .any(|candidate| matches_req(&candidate.version, &d.version_req))
                {
                    let seen: Vec<String> = eligible
                        .iter()
                        .map(|c| {
                            format!(
                                "{} at {}-{}",
                                c.version, c.toolchain.name, c.toolchain.version
                            )
                        })
                        .collect();
                    return Err(format!(
                        "{}={} requires {role} {} {} but co-selected {}",
                        p.name,
                        p.version,
                        d.name,
                        d.version_req,
                        seen.join(", ")
                    ));
                }
            }
        }
    }
    Ok(())
}

/// `{class}[{toolchain}] Name vX`, then ` w/ ` and the resolved versionsuffix.
pub fn pr_title(ecs: &[ResolvedEasyconfig]) -> String {
    fn mode(items: &[String]) -> String {
        let mut ranked: Vec<(usize, &str)> = Vec::new();
        for item in items {
            if let Some(row) = ranked.iter_mut().find(|(_, seen)| *seen == item.as_str()) {
                row.0 += 1;
            } else {
                ranked.push((1, item.as_str()));
            }
        }
        let best = ranked.iter().map(|(count, _)| *count).max().unwrap_or(0);
        ranked.retain(|(count, _)| *count == best);
        ranked.sort_by(|left, right| left.1.cmp(right.1));
        let labels: Vec<_> = ranked.into_iter().map(|(_, item)| item).collect();
        labels.join(",")
    }
    let classes = ecs
        .iter()
        .map(|ec| ec.moduleclass.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    let tc = ecs
        .iter()
        .map(|ec| format!("{}/{}", ec.toolchain.name, ec.toolchain.version))
        .collect::<Vec<_>>();
    let mut names = Vec::new();
    for ec in ecs {
        let name = format!("{} v{}", ec.name, ec.version);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.len() > 3 {
        names.truncate(3);
        names.push("...".into());
    }
    let mut title = format!("{}[{}] {}", mode(&classes), mode(&tc), names.join(", "));
    let mut suffixes = Vec::new();
    for ec in ecs {
        let raw = ec.versionsuffix.as_deref().unwrap_or("");
        if raw.is_empty() {
            continue;
        }
        let suffix = raw.trim_start_matches('-').replace('-', " ");
        if !suffixes.contains(&suffix) {
            suffixes.push(suffix);
        }
    }
    suffixes.sort();
    if suffixes.len() > 2 {
        suffixes.truncate(2);
        suffixes.push("...".into());
    }
    if !suffixes.is_empty() {
        title.push_str(" w/ ");
        title.push_str(&suffixes.join(", "));
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pr_title_expands_a_versionsuffix_template() {
        let src = "name = 'Bazel'\nversion = '7.7.1'\nversionsuffix = '-Java-%(javaver)s'\n\
toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
dependencies = [('Java', '21', '', SYSTEM)]\nmoduleclass = 'devel'\n";
        let recipe = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(recipe.versionsuffix.as_deref(), Some("-Java-21"));
        assert_eq!(
            pr_title(std::slice::from_ref(&recipe)),
            "devel[GCCcore/15.2.0] Bazel v7.7.1 w/ Java 21"
        );
    }

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

    fn existing_versions_universe() -> Vec<Candidate> {
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        let older = Toolchain {
            name: "GCCcore".into(),
            version: "14.3.0".into(),
        };
        let mk = |name: &str, version: &str, tc: &Toolchain| Candidate {
            easyblock: None,
            name: name.into(),
            version: version.into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: String::new(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        };
        vec![
            mk("CMake", "4.2.1", &gcccore),
            mk("CMake", "3.31.10", &gcccore),
            mk("CMake", "3.30.0", &older),
            mk("Boost", "1.88.0", &older),
        ]
    }

    #[test]
    fn existing_versions_returns_every_version_at_the_queried_generation() {
        let universe = existing_versions_universe();
        let generation = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        let mut got = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "CMake",
            generation: &generation,
        });
        got.sort();
        assert_eq!(got, vec!["3.31.10".to_string(), "4.2.1".to_string()]);
    }

    #[test]
    fn existing_versions_is_empty_for_a_package_not_in_the_universe() {
        let universe = existing_versions_universe();
        let generation = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        assert!(existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "LLVM",
            generation: &generation,
        })
        .is_empty());
    }

    #[test]
    fn existing_versions_excludes_other_generations_of_the_same_package() {
        let universe = existing_versions_universe();
        let generation = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        let got = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "Boost",
            generation: &generation,
        });
        assert!(
            got.is_empty(),
            "Boost 1.88.0 sits at GCCcore-14.3.0 and must not answer a GCCcore-15.2.0 query, got {got:?}"
        );
    }

    #[test]
    fn strip_comments_preserves_hashes_in_strings() {
        let s = "name = 'foo#bar'\n# comment\nversion = '1.0'\n";
        let t = strip_comments(s);
        assert!(t.contains("foo#bar"));
        assert!(!t.contains("comment"));
    }

    #[test]
    fn a_hash_inside_a_multiline_description_is_not_a_comment() {
        let src = "name = 'Doxygen'\nversion = '1.8.11'\n\
                   description = \"\"\"documents C, Java,\n C#, and Fortran\"\"\"\n\
                   toolchain = {'name': 'foss', 'version': '2016b'}\n";
        let parsed = resolve_easyconfig_str(src).expect("C# must not end the string");
        assert_eq!(parsed.toolchain.name, "foss");
    }

    #[test]
    fn a_dependency_defines_the_version_templates_a_recipe_uses() {
        let src = "name = 'dammit'\nversion = '0.3.2'\n\
                   toolchain = {'name': 'intel', 'version': '2017a'}\n\
                   versionsuffix = '-Python-%(pyver)s'\n\
                   dependencies = [('Python', '2.7.13'), ('matplotlib', '2.0.2', '-Python-%(pyver)s-Qt-4.8.7')]\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(parsed.versionsuffix.as_deref(), Some("-Python-2.7.13"));
        assert_eq!(
            parsed.dependencies[1].versionsuffix.as_deref(),
            Some("-Python-2.7.13-Qt-4.8.7")
        );
    }

    #[test]
    fn percent_formatting_collapses_a_doubled_percent() {
        let src = "name = 'matplotlib'\nversion = '2.0.2'\n\
                   toolchain = {'name': 'intel', 'version': '2017a'}\n\
                   local_qtver = '4.8.7'\n\
                   versionsuffix = '-Python-%%(pyver)s-Qt-%s' % local_qtver\n\
                   dependencies = [('Python', '2.7.13')]\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(
            parsed.versionsuffix.as_deref(),
            Some("-Python-2.7.13-Qt-4.8.7")
        );
    }

    #[test]
    fn an_fstring_version_is_read() {
        let src = "name = 'Java'\n_java_version = '11'\n_patch_version = '27'\n\
                   version = f'{_java_version}.0.{_patch_version}'\n\
                   toolchain = SYSTEM\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(parsed.version, "11.0.27");
    }

    #[test]
    fn a_slice_and_an_index_are_read() {
        let src = "name = 'protobuf-python'\nversion = '6.31.1'\n\
                   patchlevels = ['1', '2']\n\
                   toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
                   versionsuffix = '-%s' % patchlevels[0]\n\
                   dependencies = [('protobuf', version[2:])]\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(parsed.dependencies[0].version, "31.1");
        assert_eq!(parsed.versionsuffix.as_deref(), Some("-1"));
    }

    #[test]
    fn an_index_outside_the_value_is_refused_rather_than_clamped() {
        let src = "name = 'X'\nversion = '1'\nlevels = ['a']\n\
                   toolchain = SYSTEM\nversionsuffix = '-%s' % levels[3]\n";
        assert!(resolve_easyconfig_str(src).is_err());
    }

    #[test]
    fn a_dependency_list_written_with_braces_is_a_set_not_a_dict() {
        let src = "name = 'EZC3D'\nversion = '1.6.0'\n\
                   toolchain = {'name': 'gfbf', 'version': '2024a'}\n\
                   dependencies = {\n    ('Python', '3.12.3'),\n    ('SciPy-bundle', '2024.05'),\n}\n";
        let parsed = resolve_easyconfig_str(src).expect("a set of tuples is a dependency list");
        let names: Vec<&str> = parsed
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, vec!["Python", "SciPy-bundle"]);
    }

    #[test]
    fn an_arch_guard_appends_a_dependency_only_for_this_machine() {
        let host = std::env::consts::ARCH;
        let other = if host == "aarch64" {
            "x86_64"
        } else {
            "aarch64"
        };
        // Spaces after `\n` are the easyconfig indent. A `\` continuation
        // would strip them, and an unindented body is not a body.
        let src = format!(
            "name = 'ParaView'\nversion = '6.1.1'\n\
             toolchain = {{'name': 'foss', 'version': '2026.1'}}\n\
             dependencies = [('XZ', '5.8.2')]\n\
             if ARCH == '{host}':\n    # kept on this machine\n    \
             dependencies.append(('OSPRay', '2.12.0', '', SYSTEM))\n\
             if ARCH == '{other}':\n    \
             dependencies.append(('NotHere', '1.0'))\n\
             moduleclass = 'vis'\n"
        );
        let parsed = resolve_easyconfig_str(&src).expect("parse");
        let names: Vec<&str> = parsed
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, vec!["XZ", "OSPRay"]);
        let tc = parsed.dependencies[1]
            .toolchain
            .as_ref()
            .expect("appended dependency pins SYSTEM");
        assert_eq!(tc.name, "system");
        assert_eq!(parsed.moduleclass.as_deref(), Some("vis"));
    }

    #[test]
    fn a_dependency_list_can_be_two_lists_added() {
        let src = "name = 'PennyLane'\nversion = '0.37.0'\n\
                   toolchain = {'name': 'foss', 'version': '2023a'}\n\
                   local_extra = [('pybind11', '2.11.1')]\n\
                   builddependencies = [('CMake', '3.26.3')] + local_extra\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        let names: Vec<&str> = parsed
            .builddependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, vec!["CMake", "pybind11"]);
    }

    #[test]
    fn arch_is_defined_as_a_bare_name() {
        let src = "name = 'IJulia'\nversion = '1.29.0'\ntoolchain = SYSTEM\n\
                   dependencies = [('Julia', '1.11.6', '-linux-%s' % ARCH, SYSTEM)]\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(
            parsed.dependencies[0].versionsuffix.as_deref(),
            Some(format!("-linux-{}", std::env::consts::ARCH).as_str())
        );
    }

    #[test]
    fn explicit_arch_parsing_catches_missing_local_arch_keys() {
        let src = "name = 'Go'\nversion = '1.26.7'\ntoolchain = SYSTEM\n\
                   local_archs = {'aarch64': 'arm64', 'x86_64': 'amd64'}\n\
                   sources = ['go%%(version)s.linux-%s.tar.gz' % local_archs[ARCH]]\n";
        let x86 = resolve_easyconfig_str_for_arch(src, "x86_64").expect("x86_64 parse");
        assert_eq!(x86.name, "Go");
        let riscv = resolve_easyconfig_str_for_arch(src, "riscv64");
        assert!(
            riscv.is_err(),
            "riscv64 must fail when local_archs lacks it"
        );
    }

    /// Go, Anaconda3, Miniforge3 and cuTENSOR name the tarball with a
    /// per-architecture dict and a `%s` source list. `.get(ARCH, ARCH)`
    /// keeps the architecture name when the dict has no entry, which is how
    /// the riscv64 tarball is spelled. A source line that still cannot be
    /// evaluated stays a parse failure.
    #[test]
    fn a_per_arch_sources_dict_and_a_templated_source_list_are_read() {
        let src = "name = 'Go'\nversion = '1.26.7'\ntoolchain = SYSTEM\n\
                   local_arch = {'aarch64': 'arm64', 'x86_64': 'amd64'}.get(ARCH, ARCH)\n\
                   sources = ['go%%(version)s.linux-%s.tar.gz' % local_arch]\n";
        let x86 = resolve_easyconfig_str_for_arch(src, "x86_64").expect("x86_64");
        assert_eq!(x86.source_files[0].filename, "go1.26.7.linux-amd64.tar.gz");
        let arm = resolve_easyconfig_str_for_arch(src, "aarch64").expect("aarch64");
        assert_eq!(arm.source_files[0].filename, "go1.26.7.linux-arm64.tar.gz");
        let riscv = resolve_easyconfig_str_for_arch(src, "riscv64").expect("default is ARCH");
        assert_eq!(
            riscv.source_files[0].filename,
            "go1.26.7.linux-riscv64.tar.gz"
        );

        let unreadable = "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\n\
                          sources = [version.center(4)]\n";
        let err = resolve_easyconfig_str(unreadable).unwrap_err().to_string();
        assert!(
            err.contains("sources is assigned but could not be read"),
            "{err}"
        );
    }

    /// Java builds the tarball name with `str.format`. Kaleido picks the
    /// manylinux tag in the `else` of an `ARCH` guard. QuantumESPRESSO takes
    /// a short commit hash with an f-string slice.
    #[test]
    fn format_else_and_fstring_slices_read_real_source_names() {
        let java = "name = 'Java'\n\
                    _java_version = '21'\n\
                    _patch_version = '2'\n\
                    _build_version = '13'\n\
                    version = f'{_java_version}.0.{_patch_version}'\n\
                    toolchain = SYSTEM\n\
                    _tarball_tmpl = f'OpenJDK{_java_version}U-jdk_{{}}_linux_hotspot_{version}_{_build_version}.tar.gz'\n\
                    sources = [_tarball_tmpl.format('%(jdkarch)s')]\n";
        let parsed = resolve_easyconfig_str(java).expect("Java format");
        assert_eq!(
            parsed.source_files[0].filename,
            "OpenJDK21U-jdk_%(jdkarch)s_linux_hotspot_21.0.2_13.tar.gz"
        );

        let kaleido = "name = 'Kaleido'\nversion = '0.2.1'\n\
                       toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
                       if ARCH == 'aarch64':\n    \
                       local_manylinux = 'manylinux2014'\n\
                       else:\n    \
                       local_manylinux = 'manylinux1'\n\
                       sources = [f'%(namelower)s-%(version)s-py2.py3-none-{local_manylinux}_%(arch)s.whl']\n";
        let x86 = resolve_easyconfig_str_for_arch(kaleido, "x86_64").expect("Kaleido else");
        assert_eq!(
            x86.source_files[0].filename,
            "kaleido-0.2.1-py2.py3-none-manylinux1_x86_64.whl"
        );
        let arm = resolve_easyconfig_str_for_arch(kaleido, "aarch64").expect("Kaleido if");
        assert_eq!(
            arm.source_files[0].filename,
            "kaleido-0.2.1-py2.py3-none-manylinux2014_aarch64.whl"
        );

        let qe = "name = 'QuantumESPRESSO'\nversion = '7.5'\n\
                  toolchain = {'name': 'foss', 'version': '2025b'}\n\
                  local_lapack_hash = '12d825396fcef1e0a1b27be9f119f9e554621e55'\n\
                  sources = [{'filename': f'lapack-{local_lapack_hash[:8]}.tar.xz'}]\n";
        let parsed = resolve_easyconfig_str(qe).expect("QE slice");
        assert_eq!(parsed.source_files[0].filename, "lapack-12d82539.tar.xz");

        // RDKit's extract command is parenthesized adjacent f-strings. The
        // second piece begins with `f`, so it is not a bare quote.
        let rdkit = "name = 'RDKit'\nversion = '2026.03.4'\n\
                     toolchain = {'name': 'foss', 'version': '2025b'}\n\
                     _version = version.replace('.', '_')\n\
                     _ext = f'%(builddir)s/rdkit-Release_{_version}/External/INCHI-API'\n\
                     sources = [\n\
                         f'Release_{_version}.tar.gz',\n\
                         {\n\
                             'filename': 'INCHI-1-SRC.zip',\n\
                             'extract_cmd': (\n\
                                 f'rm -rf {_ext}/src && '\n\
                                 f'unzip -q %s -d {_ext}'\n\
                             ),\n\
                         },\n\
                     ]\n";
        let parsed = resolve_easyconfig_str(rdkit).expect("RDKit f-strings");
        assert_eq!(parsed.source_files[0].filename, "Release_2026_03_4.tar.gz");
        assert_eq!(parsed.source_files[1].filename, "INCHI-1-SRC.zip");
    }

    /// EvidentialGene indexes a month table with `int()`. Geant4 and PovChem
    /// build `sources` with a comprehension. SPM unpacks one version into the
    /// two names the tarballs use.
    #[test]
    fn int_unpack_and_source_comprehensions_are_read() {
        let evigene = "name = 'EvidentialGene'\nversion = '2018.01.01'\n\
                       toolchain = SYSTEM\n\
                       local_month = ['', 'jan', 'feb', 'mar', 'apr', 'may', 'jun', \
                       'jul', 'aug', 'sep', 'oct', 'nov', 'dec']\n\
                       local_dlver = version.split('.')[0][-2:] + \
                       local_month[int(version.split('.')[1])] + version.split('.')[2]\n\
                       sources = ['evigene%s.tar' % local_dlver]\n";
        let parsed = resolve_easyconfig_str(evigene).expect("EvidentialGene");
        assert_eq!(parsed.source_files[0].filename, "evigene18jan01.tar");

        let geant = "name = 'Geant4-data'\nversion = '11.1'\ntoolchain = SYSTEM\n\
                     local_datasets = [\n\
                         ('G4NDL', '4.7', 'G4NDL', 'G4NEUTRONHPDATA'),\n\
                         ('G4EMLOW', '8.2', 'G4EMLOW', 'G4LEDATA'),\n\
                     ]\n\
                     sources = ['%s.%s.tar.gz' % (x[2], x[1]) for x in local_datasets]\n";
        let parsed = resolve_easyconfig_str(geant).expect("Geant4-data");
        assert_eq!(parsed.source_files[0].filename, "G4NDL.4.7.tar.gz");
        assert_eq!(parsed.source_files[1].filename, "G4EMLOW.8.2.tar.gz");

        let spm = "name = 'SPM'\nversion = '12.5_r7771'\ntoolchain = SYSTEM\n\
                   _spm_majmin_ver, _spm_update_ver = version.split('_')\n\
                   sources = ['%%(namelower)s%s.zip' % _spm_majmin_ver]\n";
        let parsed = resolve_easyconfig_str(spm).expect("SPM");
        assert_eq!(parsed.source_files[0].filename, "spm12.5.zip");

        let povchem = "name = 'PovChem1'\nversion = '1.0'\ntoolchain = SYSTEM\n\
                       _source_list = ['povchem.c', 'povchem.cfg']\n\
                       sources = [{'filename': x, 'extract_cmd': f'cp %s {x}'} \
                       for x in _source_list]\n";
        let parsed = resolve_easyconfig_str(povchem).expect("PovChem");
        assert_eq!(parsed.source_files[0].filename, "povchem.c");
        assert_eq!(parsed.source_files[1].filename, "povchem.cfg");
    }

    #[test]
    fn a_versionsuffix_built_with_string_methods_is_read() {
        let src = "name = 'PyZMQ'\nversion = '2.2.0.1'\n\
                   toolchain = {'name': 'goolf', 'version': '1.4.10'}\n\
                   zmqversion = '3.2.2'\npythonversion = '2.7.3'\n\
                   versionsuffix = '-Python-%s-%s' % (pythonversion, 'zmq%s' % zmqversion.split('.')[0])\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(parsed.versionsuffix.as_deref(), Some("-Python-2.7.3-zmq3"));
    }

    #[test]
    fn join_and_replace_read_the_way_python_does() {
        let src = "name = 'X'\nversion = '3.11.2'\ntoolchain = SYSTEM\n\
                   versionsuffix = '-' + '.'.join(version.split('.')[:2]) + '-' + version.replace('.', '_')\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(parsed.versionsuffix.as_deref(), Some("-3.11-3_11_2"));
    }

    #[test]
    fn an_unsupported_method_is_refused_rather_than_guessed() {
        let src = "name = 'X'\nversion = '1.0'\ntoolchain = SYSTEM\n\
                   versionsuffix = '-%s' % version.center(9)\n";
        assert!(resolve_easyconfig_str(src).is_err());
    }

    #[test]
    fn an_arch_specific_dependency_version_resolves_for_this_machine() {
        let src = "name = 'Java'\nversion = '1.8'\ntoolchain = SYSTEM\n\
                   dependencies = [('Java', {'arch=x86_64': '%(version)s.0_311',\n\
                   'arch=POWER': '%(version)s_191-b26-OpenJDK'})]\n";
        let parsed = resolve_easyconfig_str(src).expect("parse");
        let want = match std::env::consts::ARCH {
            "x86_64" => "1.8.0_311",
            _ => "1.8_191-b26-OpenJDK",
        };
        assert_eq!(parsed.dependencies[0].version, want);
    }

    #[test]
    fn a_true_dependency_toolchain_means_the_system_one() {
        let src = "name = 'XZ'\nversion = '5.2.4'\n\
                   toolchain = {'name': 'GCCcore', 'version': '8.2.0'}\n\
                   builddependencies = [('gettext', '0.19.8.1', '', True)]\n";
        let parsed = resolve_easyconfig_str(src).expect("True is the old spelling of SYSTEM");
        let tc = parsed.builddependencies[0]
            .toolchain
            .as_ref()
            .expect("a toolchain");
        assert_eq!(tc.name, "system");
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
    fn validate_lock_deps_checks_the_variant_the_lock_names() {
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let at = |name: &str, version: &str, suffix: Option<&str>, deps: Vec<DepReq>| Candidate {
            easyblock: None,
            name: name.into(),
            version: version.into(),
            toolchain: system.clone(),
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: format!("{name}-{version}{}.eb", suffix.unwrap_or("")),
            dependencies: deps,
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        };
        let java = |version: &str| DepReq {
            name: "Java".into(),
            version_req: format!("=={version}"),
            versionsuffix: None,
            toolchain: None,
        };
        let ant17 = at("ant", "1.10.15", Some("-Java-17"), vec![java("17")]);
        let ant21 = at("ant", "1.10.15", Some("-Java-21"), vec![java("21")]);
        let java21 = at("Java", "21", None, vec![]);
        let lock = lock_from_candidates(&[ant21.clone(), java21.clone()], None, "test");
        assert!(validate_lock_deps(&lock, &[ant17, ant21, java21]).is_ok());
    }

    #[test]
    fn validate_lock_deps_requires_builddependencies() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let root = Candidate {
            easyblock: None,
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
            moduleclass: None,
        };
        let tool = Candidate {
            easyblock: None,
            name: "Tool".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "Tool-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
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
                patches: Vec::new(),
            }],
            easyconfig_path: "App.eb".into(),
            easyblock: None,
            configopts: None,
            moduleclass: None,
            homepage: None,
            checksums: vec![],
            sources_count: 0,
            source_urls: Vec::new(),
            source_files: Vec::new(),
            patch_names: vec![],
            checksum_entry_keys: vec![],
            checksums_by_filename: Default::default(),
            has_components: false,
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
    fn a_parse_error_from_a_file_names_the_file() {
        // resolve_easyconfig_file rewrites the placeholder with the real path,
        // so a caller is told which recipe failed rather than "<string>".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Broken-1.0.eb");
        std::fs::write(&path, "version = '1.0'\ntoolchain = SYSTEM\n").unwrap();
        let msg = resolve_easyconfig_file(&path).unwrap_err().to_string();
        assert!(msg.contains("Broken-1.0.eb"), "no file in {msg:?}");
        assert!(!msg.contains("<string>"), "placeholder survived: {msg:?}");
    }

    #[test]
    fn a_bad_span_reports_a_line_instead_of_panicking() {
        // The lexer sites that used to unwrap now report. The statement loop
        // is deliberately tolerant and swallows these, so this exercises the
        // helper directly: the value is that a malformed span cannot abort a
        // process that embeds the parser.
        let mut parser = Parser::new("name = 'X'\nversion = '1'\n", host_arch());
        parser.pos = 12;
        let err = parser.slice_str(usize::MAX).unwrap_err();
        assert!(err.starts_with("line "), "no line number in {err:?}");
        assert!(err.contains("outside the source"), "{err:?}");
    }

    /// One recipe, both modes: tolerant still resolves the rest, strict names
    /// what it lost. This is the pair the --strict flag exists to expose.
    #[test]
    fn strict_reports_what_tolerant_silently_skips() {
        let src = concat!(
            "name = 'X'\n",
            "version = '1'\n",
            "toolchain = SYSTEM\n",
            "broken = = =\n",
            "moduleclass = 'lib'\n",
        );

        // Tolerant: the malformed line costs nothing but itself.
        let tolerant = resolve_easyconfig_str(src).expect("tolerant parse");
        assert_eq!(tolerant.name, "X");
        assert_eq!(tolerant.version, "1");
        assert_eq!(
            tolerant.moduleclass.as_deref(),
            Some("lib"),
            "a later statement must survive an earlier skip"
        );

        // Reporting: same resolution, plus the record of what was dropped.
        let (reported, skipped) = resolve_easyconfig_str_reporting(src).expect("reporting parse");
        assert_eq!(reported, tolerant, "reporting must not change the parse");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].line, 4, "{:?}", skipped[0]);
        assert!(skipped[0].text.starts_with("broken"), "{:?}", skipped[0]);
        assert!(
            skipped[0].message.contains("line "),
            "the message carries its own position: {:?}",
            skipped[0]
        );
        let rendered = skipped[0].to_string();
        assert!(rendered.contains("broken"), "{rendered}");
        assert_eq!(
            rendered.matches("line ").count(),
            1,
            "the position is printed once, not repeated from the message: {rendered}"
        );
        assert!(
            rendered.contains(skipped[0].reason()),
            "the reason survives the de-duplication: {rendered}"
        );
    }

    #[test]
    fn a_clean_recipe_has_nothing_to_report() {
        let src = "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\n";
        let (_resolved, skipped) = resolve_easyconfig_str_reporting(src).expect("parse");
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn a_control_block_is_not_reported_as_a_failure() {
        // if/for blocks are skipped deliberately, not as errors. Reporting
        // them would bury the real signal in noise.
        let src = concat!(
            "name = 'X'\n",
            "version = '1'\n",
            "toolchain = SYSTEM\n",
            "if True:\n",
            "    dependencies = [('zlib', '1.3')]\n",
            "moduleclass = 'lib'\n",
        );
        let (resolved, skipped) = resolve_easyconfig_str_reporting(src).expect("parse");
        assert_eq!(resolved.moduleclass.as_deref(), Some("lib"));
        assert!(
            skipped.is_empty(),
            "a control block is a deliberate skip, not a parse failure: {skipped:?}"
        );
    }

    #[test]
    fn a_malformed_statement_is_skipped_rather_than_failing_the_recipe() {
        // Tolerance is the design: one unrepresentable statement must not cost
        // the whole easyconfig. Pinned so the unwrap removal cannot quietly
        // turn recoverable input into a hard failure.
        let src = "name = 'X'\nversion = '1'\ntoolchain = SYSTEM\nn = 99999999999999999999999\n";
        let r = resolve_easyconfig_str(src).expect("tolerant parse");
        assert_eq!(r.name, "X");
        assert_eq!(r.version, "1");
    }

    #[test]
    fn an_unknown_template_key_is_left_alone_rather_than_dropped() {
        // The substitution closure must not lose text when a key is unknown;
        // losing it silently corrupted the value instead of reporting.
        let mut templates = HashMap::new();
        templates.insert("version".to_string(), "1.2.3".to_string());
        assert_eq!(
            apply_templates_str("pkg-%(version)s-%(unknown)s.tar.gz", &templates),
            "pkg-1.2.3-%(unknown)s.tar.gz"
        );
        assert_eq!(
            apply_templates_str("no templates", &templates),
            "no templates"
        );
    }

    #[test]
    fn the_template_pattern_is_compiled_once() {
        // Same instance across calls: the pattern used to be rebuilt on every
        // invocation, eight times per value in the nested-substitution loop.
        let first = template_pattern() as *const regex::Regex;
        let second = template_pattern() as *const regex::Regex;
        assert_eq!(first, second);
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
    fn recipe_github_account_replaces_the_namelower_default() {
        // The assignment sits below source_urls on purpose: templating runs
        // after the whole file is read, as it does in EasyBuild.
        let src = r#"
name = 'Widget'
version = '1.2.3'
toolchain = SYSTEM
sources = [{'filename': 'v%(version)s.tar.gz', 'source_urls': [GITHUB_SOURCE]}]
source_urls = [GITHUB_LOWER_SOURCE]
github_account = 'some-org'
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        assert_eq!(
            r.source_urls,
            vec!["https://github.com/some-org/widget/archive"]
        );
        assert_eq!(
            r.source_files[0].source_urls,
            vec!["https://github.com/some-org/Widget/archive"]
        );
        assert_eq!(r.source_files[0].download_filename, "v1.2.3.tar.gz");
    }

    #[test]
    fn github_account_defaults_to_namelower_when_the_recipe_omits_it() {
        let src = r#"
name = 'Widget'
version = '1.2.3'
toolchain = SYSTEM
source_urls = [GITHUB_SOURCE]
sources = [SOURCE_TAR_GZ]
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        assert_eq!(
            r.source_urls,
            vec!["https://github.com/widget/Widget/archive"]
        );
    }

    #[test]
    fn an_explicit_forge_account_replaces_the_lowercased_name() {
        // eOn's recipe names TheochemUI. Leaving github_account at "eon" makes
        // GITHUB_SOURCE a different archive than the one the checksum belongs to.
        let src = r#"
name = 'eOn'
version = '2.17.10'
toolchain = {'name': 'foss', 'version': '2026.1'}
github_account = 'TheochemUI'
bitbucket_account = '%(namelower)s'
versionprefix = 'v'
source_urls = [GITHUB_SOURCE, BITBUCKET_SOURCE]
homepage = 'https://example.invalid/%(github_account)s/%(versionprefix)s%(version)s'
"#;
        let resolved = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(
            resolved.source_urls,
            vec![
                "https://github.com/TheochemUI/eOn/archive".to_string(),
                "https://bitbucket.org/eon/eon/get".to_string(),
            ]
        );
        assert_eq!(
            resolved.homepage.as_deref(),
            Some("https://example.invalid/TheochemUI/v2.17.10")
        );
    }

    #[test]
    fn recipe_bitbucket_account_replaces_the_namelower_default() {
        let src = r#"
name = 'Widget'
version = '1.2.3'
toolchain = SYSTEM
source_urls = [BITBUCKET_SOURCE]
bitbucket_account = '%(namelower)s-team'
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        assert_eq!(
            r.source_urls,
            vec!["https://bitbucket.org/widget-team/widget/get"]
        );
    }

    #[test]
    fn forge_accounts_default_to_the_lowercased_name() {
        let src = r#"
name = 'eOn'
version = '1'
toolchain = SYSTEM
source_urls = [GITHUB_SOURCE, BITBUCKET_SOURCE]
"#;
        let resolved = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(
            resolved.source_urls,
            vec![
                "https://github.com/eon/eOn/archive".to_string(),
                "https://bitbucket.org/eon/eon/get".to_string(),
            ]
        );
        assert_eq!(resolved.homepage, None);
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
    fn missing_bare_tuple_does_not_call_system_a_generation() {
        let recipe = resolve_easyconfig_str(
            "name = 'App'\nversion = '1.0'\n\
             toolchain = {'name': 'foss', 'version': '2025b'}\n\
             dependencies = [('Java', '17')]\n",
        )
        .expect("parse");
        let java = resolve_easyconfig_str(
            "name = 'Java'\nversion = '17'\n\
             toolchain = SYSTEM\n\
             dependencies = []\n",
        )
        .expect("parse")
        .to_candidate();
        let check = check_recipe_deps(&recipe, &[java]);
        assert!(!check.ok());
        let reason = &check.missing[0].reason;
        assert!(
            reason.contains("a bare tuple does not reach SYSTEM"),
            "{reason}"
        );
        assert!(reason.contains("('Java', '17', '', SYSTEM)"), "{reason}");
        assert!(!reason.contains("system <"), "{reason}");
        assert!(!reason.contains("@ SYSTEM"), "{reason}");
        assert!(
            !reason.contains("available at other generations"),
            "{reason}"
        );
        assert!(
            reason.contains("GCCcore-14.3.0"),
            "compiler members stay in the note: {reason}"
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
                easyblock: None,
                name: "Lib".into(),
                version: "1.0".into(),
                toolchain: tc.clone(),
                versionsuffix: None,
                easyconfig_path: "upstream/Lib-1.0.eb".into(),
                dependencies: vec![],
                builddependencies: vec![],
                exts_list: vec![],
                moduleclass: None,
            },
            Candidate {
                easyblock: None,
                name: "Keep".into(),
                version: "2.0".into(),
                toolchain: tc.clone(),
                versionsuffix: None,
                easyconfig_path: "upstream/Keep-2.0.eb".into(),
                dependencies: vec![],
                builddependencies: vec![],
                exts_list: vec![],
                moduleclass: None,
            },
        ];
        let overlay = vec![Candidate {
            easyblock: None,
            name: "Lib".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "overlay/Lib-1.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
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
            easyblock: None,
            name: "PyTorch".into(),
            version: "2.9.1".into(),
            toolchain: tc.clone(),
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: path.into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
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
            source_urls: Vec::new(),
            source_files: Vec::new(),
            patch_names: vec![],
            checksum_entry_keys: vec![],
            checksums_by_filename: Default::default(),
            has_components: false,
        }
    }

    #[test]
    fn easyblock_sites_keep_extension_and_component_classes() {
        let src = r#"
name = 'BundleApp'
version = '1.0'
homepage = 'https://example.invalid'
description = 'bundle'
toolchain = {'name': 'foss', 'version': '2026.1'}
easyblock = 'PythonBundle'
exts_defaultclass = 'PythonPackage'
exts_classmap = {'special': 'Cargo'}
exts_list = [
    ('plain', '1.0'),
    ('custom', '2.0', {'easyblock': 'PerlModule'}),
]
default_easyblock = 'ConfigureMake'
components = [
    ('zlib', '1.2', {'easyblock': 'CMakeMake'}),
    ('bare', '0.1'),
]
moduleclass = 'tools'
"#;
        let (recipe, sites) = easyblock_sites(src).expect("parse");
        assert_eq!(recipe.easyblock.as_deref(), Some("PythonBundle"));
        assert_eq!(sites.exts_defaultclass.as_deref(), Some("PythonPackage"));
        assert_eq!(sites.default_easyblock.as_deref(), Some("ConfigureMake"));
        assert_eq!(
            sites.exts_classmap.get("special").map(String::as_str),
            Some("Cargo")
        );
        assert_eq!(sites.extensions.len(), 2);
        assert_eq!(sites.extensions[0].class_name, None);
        assert_eq!(
            sites.extensions[1].class_name.as_deref(),
            Some("PerlModule")
        );
        assert_eq!(sites.components[0].class_name.as_deref(), Some("CMakeMake"));
        assert_eq!(sites.components[1].name, "bare");
        assert_eq!(sites.components[1].class_name, None);
    }

    #[test]
    fn a_skipped_component_list_keeps_its_easyblock_literal() {
        let src = r#"
name = 'GFE'
version = '1.28.0'
homepage = 'https://example.invalid'
description = 'bundle'
toolchain = SYSTEM
easyblock = 'Bundle'
default_easyblock = 'Tarball'
_libs = {'gftl': ('1.17.0', 'abc')}
components = [
    (_name, _version, {'checksums': [_checksum]}) for _name, (_version, _checksum) in _libs.items()
] + [
    (name, version, {
        'preconfigopts': ' && '.join([f'rm {_name}' for _name in _libs]) + ' && ',
        'easyblock': 'CMakeMake',
        'runtest': 'tests',
    }),
]
moduleclass = 'tools'
"#;
        let (recipe, sites) = easyblock_sites(src).expect("parse");
        assert_eq!(recipe.easyblock.as_deref(), Some("Bundle"));
        assert_eq!(sites.default_easyblock.as_deref(), Some("Tarball"));
        assert!(
            sites
                .components
                .iter()
                .any(|row| row.class_name.as_deref() == Some("CMakeMake")),
            "components: {:?}",
            sites.components
        );
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
            easyblock: None,
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
            moduleclass: None,
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
            easyblock: None,
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
            moduleclass: None,
        };
        let xtb = Candidate {
            easyblock: None,
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
            moduleclass: None,
        };
        // Minimal foss-2026.1 tree so hierarchy derives with GCCcore-15.2.0.
        let mut foss_def = Candidate {
            easyblock: None,
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
            moduleclass: None,
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
    fn unpinned_dep_does_not_fall_through_to_system_toolchain() {
        let mut recipe = blank_recipe();
        recipe.toolchain = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        recipe.builddependencies = vec![ResolvedDep {
            name: "BuildTool".into(),
            version: "3.0".into(),
            versionsuffix: None,
            toolchain: None,
        }];
        let system_only = Candidate {
            easyblock: None,
            name: "BuildTool".into(),
            version: "3.0".into(),
            toolchain: Toolchain {
                name: "system".into(),
                version: String::new(),
            },
            versionsuffix: None,
            easyconfig_path: "BuildTool-3.0.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        };

        let check = check_recipe_deps(&recipe, &[system_only]);
        assert!(
            check.missing.iter().any(|missing| missing.name == "BuildTool"),
            "an implicit dependency of a non-system recipe cannot select a SYSTEM recipe: {check:?}"
        );
    }

    /// A recipe on a toolchain with no derivable hierarchy matches its deps by
    /// name and version alone, which can pair it with another generation
    /// entirely. That has to be visible: it is how a Wannier90 at foss-2022a
    /// got reported as satisfying a dependency of an NVHPC-toolchain recipe.
    #[test]
    fn check_recipe_deps_flags_matches_made_without_a_hierarchy() {
        let mut r = blank_recipe();
        // No defining recipe for this generation anywhere in the universe.
        r.toolchain = Toolchain {
            name: "madeupchain".into(),
            version: "9999.99".into(),
        };
        r.dependencies = vec![ResolvedDep {
            name: "Wannier90".into(),
            version: "3.1.0".into(),
            versionsuffix: None,
            toolchain: None,
        }];
        let universe = vec![Candidate {
            easyblock: None,
            name: "Wannier90".into(),
            version: "3.1.0".into(),
            // Deliberately an unrelated generation.
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2022a".into(),
            },
            versionsuffix: None,
            easyconfig_path: "Wannier90-3.1.0-foss-2022a.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        }];
        let check = check_recipe_deps(&r, &universe);
        // Nothing is "missing", but that verdict carries no toolchain evidence.
        assert!(check.ok());
        assert!(!check.toolchain_verified());
        let note = check
            .unverified_toolchain
            .as_deref()
            .expect("degraded match must be reported");
        assert!(note.contains("madeupchain"), "got {note}");
    }

    #[test]
    fn check_recipe_deps_is_verified_when_hierarchy_is_known() {
        let mut r = blank_recipe();
        r.toolchain = Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        };
        let check = check_recipe_deps(&r, &[]);
        // foss-2025a ships as an embedded hierarchy, so matching is filtered.
        assert!(check.toolchain_verified());
        assert!(check.unverified_toolchain.is_none());
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
            easyblock: None,
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
            moduleclass: None,
        }];
        let check = check_recipe_deps(&r, &universe);
        assert!(!check.ok());
        assert!(check.found.iter().any(|f| f.contains("Python")));
        assert!(check
            .missing
            .iter()
            .any(|m| m.name == "Meson" && m.role == "build"));
    }
    #[test]
    fn aug_assign_extends_dependency_list() {
        let src = r#"
name = 'demo'
version = '1.0'
toolchain = {'name': 'foss', 'version': '2025b'}
dependencies = [('zlib', '1.3.1')]
dependencies += [('OpenBLAS', '0.3.27')]
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        let names: Vec<&str> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["zlib", "OpenBLAS"], "+= must extend, not replace");
    }

    #[test]
    fn percent_format_with_tuple_args() {
        let src = r#"
name = 'demo'
version = '2.1'
local_slug = '%s-%s' % (name, version)
homepage = 'https://example.org/%s' % local_slug
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        assert_eq!(r.homepage.as_deref(), Some("https://example.org/demo-2.1"));
    }

    #[test]
    fn percent_format_replaces_leftmost_conversion() {
        let src = r#"
name = 'demo'
version = '1.0'
homepage = 'https://example.org/v%d/%s' % (3, name)
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
"#;
        let r = resolve_easyconfig_str(src).expect("resolve");
        assert_eq!(r.homepage.as_deref(), Some("https://example.org/v3/demo"));
    }
    #[test]
    fn percent_format_arity_mismatch_skips_the_field_instead_of_guessing() {
        // Python raises TypeError for both shapes. The parser must not emit a
        // partially-substituted value: before the arity check, `too_many` silently
        // dropped `version` and `too_few` left a literal `%s` in the URL.
        let too_many = r#"
name = 'demo'
version = '1.0'
homepage = 'https://example.org/%s' % (name, version)
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
"#;
        let r = resolve_easyconfig_str(too_many).expect("file still resolves");
        assert_eq!(
            r.homepage, None,
            "extra operand must skip the field, not be dropped silently"
        );

        let too_few = r#"
name = 'demo'
version = '1.0'
homepage = 'https://example.org/%s/%s' % (name,)
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}
"#;
        let r = resolve_easyconfig_str(too_few).expect("file still resolves");
        assert_eq!(r.homepage, None, "unfilled conversion must skip the field");
    }

    #[test]
    fn percent_escape_is_not_counted_as_a_conversion() {
        // `%%` is a literal percent, so this format has exactly one real conversion.
        assert_eq!(count_percent_conversions("100%% of %s"), 1);
        assert_eq!(count_percent_conversions("%s-%s"), 2);
        assert_eq!(count_percent_conversions("no conversions here"), 0);
    }
    #[test]
    fn existing_versions_filters_by_name_and_generation_newest_first() {
        let tc = |n: &str, v: &str| Toolchain {
            name: n.into(),
            version: v.into(),
        };
        let cand = |name: &str, ver: &str, t: Toolchain| Candidate {
            easyblock: None,
            name: name.into(),
            version: ver.into(),
            toolchain: t,
            versionsuffix: None,
            easyconfig_path: String::new(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let universe = vec![
            cand("CMake", "3.31.11", tc("GCCcore", "15.2.0")),
            cand("CMake", "4.2.1", tc("GCCcore", "15.2.0")),
            cand("CMake", "3.29.3", tc("GCCcore", "13.3.0")),
            cand("Ninja", "1.12.1", tc("GCCcore", "15.2.0")),
        ];
        let gen = tc("GCCcore", "15.2.0");
        let got = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "CMake",
            generation: &gen,
        });
        // Newest first, other generations and other packages excluded.
        assert_eq!(got, vec!["4.2.1".to_string(), "3.31.11".to_string()]);

        // This is exactly what makes a backfill detectable: 3.31.11 lands
        // into a tree that already carries 4.2.1 at the same generation.
        assert!(crate::miner::is_backfill("3.31.11", &got));
        assert!(!crate::miner::is_backfill("4.3.0", &got));

        let absent = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "Nothing",
            generation: &gen,
        });
        assert!(absent.is_empty());
    }

    /// Two adjacent string literals are one string in Python, and easyconfigs
    /// lean on it to keep a long command under the line limit. Strict callers
    /// used to see the whole statement skipped.
    #[test]
    fn adjacent_string_literals_in_a_list_are_one_element() {
        let src = r#"name = 'VASP6'
version = '6.6.1'
toolchain = {'name': 'foss', 'version': '2025b'}
postinstallcmds = [
    'mkdir -p %(installdir)s/bin',
    'install -m 0755 %(builddir)s/hpc-utils/request_license_key.py '
    '%(installdir)s/bin/request_license_key',
]
"#;
        let (resolved, skipped) = resolve_easyconfig_str_reporting(src).expect("parse");
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(resolved.name, "VASP6");

        // On one line and outside brackets it is still one string, and
        // `configopts` is modelled, so the joined value itself is observable
        // rather than only the absence of a skip.
        let with_configopts = format!("{src}configopts = '--with-a ' '--with-b'\n");
        let (resolved, skipped) =
            resolve_easyconfig_str_reporting(&with_configopts).expect("parse");
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(
            resolved.configopts.as_deref(),
            Some("--with-a --with-b"),
            "adjacent literals must join into one value"
        );
    }

    /// Outside brackets Python needs the literals on one logical line, so a
    /// string opening the next statement must not be joined onto this one.
    #[test]
    fn a_string_on_the_next_line_is_not_joined_outside_brackets() {
        let src = r#"name = 'Demo'
version = '1.0'
toolchain = {'name': 'foss', 'version': '2025b'}
configopts = '--with-a'
homepage = 'https://example.invalid'
"#;
        let resolved = resolve_easyconfig_str(src).expect("parse");
        assert_eq!(resolved.configopts.as_deref(), Some("--with-a"));
        assert_eq!(
            resolved.homepage.as_deref(),
            Some("https://example.invalid"),
            "the following assignment must survive intact"
        );
    }
}

#[cfg(test)]
mod hierarchy_universe_tests {
    use super::*;
    use crate::hierarchy::known_hierarchy;

    fn candidate(name: &str, version: &str, tc_name: &str, tc_version: &str) -> Candidate {
        Candidate {
            easyblock: None,
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: tc_name.into(),
                version: tc_version.into(),
            },
            versionsuffix: None,
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            easyconfig_path: format!("x/{name}/{name}-{version}.eb"),
            exts_list: Vec::new(),
            moduleclass: None,
        }
    }

    /// The case a site hits on a new generation: a recipe at GCC takes its
    /// CMake from GCCcore, and a universe filtered to GCC alone reports its
    /// own build dependency as a missing package.
    #[test]
    fn the_universe_admits_the_subtoolchain_a_recipe_builds_against() {
        let gcc = Toolchain {
            name: "GCC".into(),
            version: "15.2.0".into(),
        };
        let all = vec![
            candidate("FlexiBLAS", "3.5.0", "GCC", "15.2.0"),
            candidate("CMake", "4.2.1", "GCCcore", "15.2.0"),
            candidate("zlib", "2.3.2", "system", ""),
            // another generation entirely, which must stay out
            candidate("CMake", "3.31.3", "GCCcore", "14.2.0"),
        ];
        let members = known_hierarchy(&gcc)
            .expect("GCC hierarchy is known")
            .members;
        let kept = filter_toolchain_hierarchy(&all, &gcc, &members);
        let names: Vec<String> = kept
            .iter()
            .map(|c| format!("{}-{}-{}", c.name, c.version, c.toolchain.name))
            .collect();
        assert!(
            names.contains(&"CMake-4.2.1-GCCcore".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"FlexiBLAS-3.5.0-GCC".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"zlib-2.3.2-system".to_string()),
            "{names:?}"
        );
        assert!(
            !names.contains(&"CMake-3.31.3-GCCcore".to_string()),
            "a different generation leaked in: {names:?}"
        );
    }

    #[test]
    fn with_no_hierarchy_it_is_the_old_filter() {
        let gcc = Toolchain {
            name: "GCC".into(),
            version: "15.2.0".into(),
        };
        let all = vec![
            candidate("FlexiBLAS", "3.5.0", "GCC", "15.2.0"),
            candidate("CMake", "4.2.1", "GCCcore", "15.2.0"),
        ];
        let kept = filter_toolchain_hierarchy(&all, &gcc, &[]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "FlexiBLAS");
    }
}
