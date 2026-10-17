//! Easyconfig lint rules that mirror what upstream enforces.
//!
//! Three checkers decide whether an easybuild-easyconfigs PR is clean, and
//! none of them is `eb --check-contrib` alone:
//!
//! - the easyconfigs CI lint, `flake8 --select F,W605,W291 --filename '*.eb'`
//!   on Python 3.8 (F541, W605, W291 here);
//! - the style half of `eb --check-contrib`, which is pycodestyle with only
//!   W291 and E741 ignored (E225, E226, E227, E228, E231 here; E501 lives in
//!   the parent module);
//! - the easyconfigs test suite (`python -s`, the `sanity_check_paths` shape,
//!   redefined parameters, local variable naming, checksum counts).
//!
//! The `EB_*` rules port the executable spec EESSI checked against those
//! tools (`fixtures/README.md` has the table, one `fixtures/lint_<rule>/` per
//! rule). Most rules work on a small Python tokenizer that understands
//! f-strings the way Python 3.12 does, so a file that only newer Pythons can
//! parse still gets linted. The rules that need values (`%` and `+` on
//! literals, dependency tuples, checksum counts, `sanity_check_paths`) use the
//! rustpython AST and are skipped when the file does not parse; E999 reports
//! that case.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use rustpython_parser::ast::{self, Ranged, Visitor};
use rustpython_parser::Parse;

use super::vocab;
use super::{severity_for, StyleFinding};

/// Inputs beyond the easyconfig text.
#[derive(Debug, Clone, Copy, Default)]
pub struct LintOptions<'a> {
    /// File name used in messages that EasyBuild words with one (`python -s`,
    /// checksum count). Defaults to `<easyconfig>`.
    pub file_name: Option<&'a str>,
    /// The version the change started from. Turns on EB_FMT_TEMPLATE_REMOVED.
    pub base: Option<&'a str>,
}

/// Python syntax error for `text`, as (line, column, message), or `None`.
///
/// EasyBuild `exec`s an easyconfig, so a file that is not valid Python never
/// gets as far as any other check.
pub fn python_syntax_error(text: &str) -> Option<(usize, usize, String)> {
    match ast::Suite::parse(text, "<easyconfig>") {
        Ok(_) => None,
        Err(error) => {
            let index = LineIndex::new(text);
            let offset = (u32::from(error.offset) as usize).min(text.len());
            let line = index.line(offset);
            let column = index.column(text, offset);
            Some((line, column, format!("SyntaxError: {}", error.error)))
        }
    }
}

pub(super) fn extend_rules(text: &str, options: &LintOptions<'_>, out: &mut Vec<StyleFinding>) {
    let file_name = options.file_name.unwrap_or("<easyconfig>");
    let index = LineIndex::new(text);
    let mut sink = Sink {
        text,
        index: &index,
        out,
    };
    let scan = Scanner::scan(text);
    let assigns = top_level_assigns(text, &scan.top);
    let mut names: HashSet<&str> = assigns.keys().map(String::as_str).collect();
    names.extend(vocab::PYTHON_CONSTANTS.iter().copied());

    trailing_whitespace(text, &mut sink);
    escape_sequences(&scan, &mut sink);
    missing_whitespace(text, &scan.top, &mut sink);
    fstring_placeholders(&scan, &mut sink);
    quote_reuse(&scan, &mut sink);
    missing_fprefix(text, &scan, &names, &mut sink);
    template_vars(text, &scan, &assigns, &mut sink);
    hardcoded_shlib(text, &scan, &mut sink);
    unknown_templates(text, &scan, &mut sink);
    let easyblock = easyblock_name(text);
    python_user_site(text, easyblock.as_deref(), file_name, &mut sink);
    redefined_params(text, &mut sink);
    local_var_naming(&assigns, easyblock.as_deref(), &mut sink);
    if let Some(base) = options.base {
        templates_removed(text, base, &mut sink);
    }

    match ast::Suite::parse(text, "<easyconfig>") {
        Ok(suite) => ast_rules(text, &suite, file_name, &mut sink),
        Err(_) => {
            if let Some((line, column, message)) = python_syntax_error(text) {
                sink.push_at(line, column, "E999", message);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// positions and output

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(offset + 1);
            }
        }
        Self { starts }
    }

    fn line(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    fn column(&self, text: &str, offset: usize) -> usize {
        let start = self.starts[self.line(offset) - 1];
        text.get(start..offset)
            .map(|prefix| prefix.chars().count())
            .unwrap_or(0)
            + 1
    }
}

struct Sink<'a> {
    text: &'a str,
    index: &'a LineIndex,
    out: &'a mut Vec<StyleFinding>,
}

impl Sink<'_> {
    fn push(&mut self, offset: usize, code: &str, message: String) {
        let line = self.index.line(offset);
        let column = self.index.column(self.text, offset);
        self.push_at(line, column, code, message);
    }

    fn push_at(&mut self, line: usize, column: usize, code: &str, message: String) {
        self.out.push(StyleFinding {
            line,
            column,
            code: code.to_string(),
            message,
            mechanical: false,
            origin: String::new(),
            severity: severity_for(code),
        });
    }
}

// ---------------------------------------------------------------------------
// tokenizer

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Name,
    Number,
    /// A plain (non-f) string literal, prefix and quotes included.
    Str,
    /// A whole f-string, prefix and quotes included.
    FStr,
    Op,
    /// End of a logical line.
    Newline,
}

#[derive(Debug, Clone, Copy)]
struct Tok {
    kind: TokKind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceKind {
    /// A plain string literal, anywhere (also inside an f-string field).
    Str,
    /// A literal run of an f-string (Python 3.12 `FSTRING_MIDDLE`).
    FMiddle,
}

#[derive(Debug, Clone, Copy)]
struct Piece {
    kind: PieceKind,
    start: usize,
    end: usize,
}

/// One `{...}` replacement field of an f-string.
#[derive(Debug, Clone, Copy)]
struct Field {
    /// Start of the expression (after `{` and leading whitespace).
    expr_start: usize,
    /// End of the expression (before `!`, `:` or `}`).
    expr_end: usize,
}

#[derive(Debug, Clone, Copy)]
struct FStringInfo {
    start: usize,
    has_field: bool,
    /// True when this f-string sits inside another f-string's field.
    nested: bool,
}

#[derive(Debug, Default)]
struct Scan {
    /// Tokens as Python 3.8 sees them: an f-string is one token.
    top: Vec<Tok>,
    pieces: Vec<Piece>,
    fields: Vec<Field>,
    fstrings: Vec<FStringInfo>,
    /// (offset, message) for EB_FSTRING_QUOTE_REUSE.
    quote_reuse: Vec<(usize, String)>,
    /// (offset of the backslash, escaped character) for W605.
    bad_escapes: Vec<(usize, char)>,
}

struct Scanner<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    fdepth: usize,
    scan: Scan,
}

const OPS3: &[&str] = &["**=", "//=", ">>=", "<<=", "..."];
const OPS2: &[&str] = &[
    "**", "//", "<<", ">>", "<=", ">=", "==", "!=", "->", ":=", "+=", "-=", "*=", "/=", "%=", "&=",
    "|=", "^=", "@=",
];
const OPS1: &[u8] = b"+-*/%@&|^~<>()[]{},:.;=!";

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn is_string_prefix(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "r" | "u" | "b" | "f" | "br" | "rb" | "fr" | "rf"
    )
}

impl<'a> Scanner<'a> {
    fn scan(src: &'a str) -> Scan {
        let mut scanner = Scanner {
            src,
            b: src.as_bytes(),
            pos: 0,
            fdepth: 0,
            scan: Scan::default(),
        };
        scanner.scan_top();
        scanner.scan
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.b.get(self.pos + ahead).copied()
    }

    fn at(&self, needle: &str) -> bool {
        self.b[self.pos..].starts_with(needle.as_bytes())
    }

    /// Advance past one UTF-8 character.
    fn advance_char(&mut self) {
        let width = self.src[self.pos..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        self.pos += width;
    }

    fn push_newline(&mut self) {
        let needed = matches!(self.scan.top.last(), Some(tok) if tok.kind != TokKind::Newline);
        if needed {
            self.scan.top.push(Tok {
                kind: TokKind::Newline,
                start: self.pos,
                end: self.pos,
            });
        }
    }

    fn skip_comment(&mut self) {
        while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
            self.pos += 1;
        }
    }

    fn scan_top(&mut self) {
        let mut depth = 0usize;
        while self.pos < self.b.len() {
            match self.b[self.pos] {
                b'\n' => {
                    if depth == 0 {
                        self.push_newline();
                    }
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' | 0x0c => self.pos += 1,
                b'\\' if matches!(self.peek(1), Some(b'\n' | b'\r')) => {
                    self.pos += 1;
                    if self.peek(0) == Some(b'\r') {
                        self.pos += 1;
                    }
                    if self.peek(0) == Some(b'\n') {
                        self.pos += 1;
                    }
                }
                b'#' => self.skip_comment(),
                _ => {
                    let tok = self.next_token(None);
                    if tok.kind == TokKind::Op {
                        match &self.src[tok.start..tok.end] {
                            "(" | "[" | "{" => depth += 1,
                            ")" | "]" | "}" => depth = depth.saturating_sub(1),
                            _ => {}
                        }
                    }
                    self.scan.top.push(tok);
                }
            }
        }
        self.push_newline();
    }

    /// Scan one token at `self.pos` (not whitespace). `in_field` is the quote
    /// of the f-string whose replacement field the token sits in.
    fn next_token(&mut self, in_field: Option<&str>) -> Tok {
        let start = self.pos;
        let byte = self.b[self.pos];
        if is_ident_start(byte) {
            while self.pos < self.b.len() && is_ident_char(self.b[self.pos]) {
                self.pos += 1;
            }
            let word = &self.src[start..self.pos];
            if matches!(self.peek(0), Some(b'\'' | b'"')) && is_string_prefix(word) {
                let lower = word.to_ascii_lowercase();
                let raw = lower.contains('r');
                return if lower.contains('f') {
                    self.scan_fstring(start, raw, in_field)
                } else {
                    self.scan_string(start, raw, in_field)
                };
            }
            return Tok {
                kind: TokKind::Name,
                start,
                end: self.pos,
            };
        }
        if byte == b'\'' || byte == b'"' {
            return self.scan_string(start, false, in_field);
        }
        if byte.is_ascii_digit()
            || (byte == b'.' && self.peek(1).is_some_and(|d| d.is_ascii_digit()))
        {
            let hex = byte == b'0' && matches!(self.peek(1), Some(b'x' | b'X'));
            while self.pos < self.b.len() {
                let ch = self.b[self.pos];
                let exponent_sign = (ch == b'+' || ch == b'-')
                    && !hex
                    && self.pos > 0
                    && matches!(self.b[self.pos - 1], b'e' | b'E');
                if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.' || exponent_sign {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            return Tok {
                kind: TokKind::Number,
                start,
                end: self.pos,
            };
        }
        for op in OPS3.iter().chain(OPS2.iter()) {
            if self.at(op) {
                self.pos += op.len();
                return Tok {
                    kind: TokKind::Op,
                    start,
                    end: self.pos,
                };
            }
        }
        if OPS1.contains(&byte) {
            self.pos += 1;
        } else {
            self.advance_char();
        }
        Tok {
            kind: TokKind::Op,
            start,
            end: self.pos,
        }
    }

    /// Quote text (`'`, `"`, `'''` or `"""`) at `self.pos`.
    fn quote_here(&self) -> &'a str {
        let src: &'a str = self.src;
        let byte = self.b[self.pos];
        let triple = self.peek(1) == Some(byte) && self.peek(2) == Some(byte);
        let len = if triple { 3 } else { 1 };
        &src[self.pos..self.pos + len]
    }

    fn check_escape(&mut self, backslash: usize, raw: bool) {
        if raw {
            return;
        }
        let Some(next) = self.src[backslash + 1..].chars().next() else {
            return;
        };
        let valid = matches!(
            next,
            '\n' | '\r' | '\\' | '\'' | '"' | 'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '0'
                ..='7' | 'x' | 'N' | 'u' | 'U'
        );
        if !valid {
            self.scan.bad_escapes.push((backslash, next));
        }
    }

    /// Record EB_FSTRING_QUOTE_REUSE for a literal that starts inside the
    /// replacement field of an f-string quoted with `in_field`. Returns true
    /// when a plain string reused the quote (its message needs the token).
    fn note_nested_quote(
        &mut self,
        start: usize,
        quote: &str,
        is_fstring: bool,
        in_field: Option<&str>,
    ) -> bool {
        let Some(outer) = in_field else {
            return false;
        };
        if is_fstring {
            if quote == outer {
                self.scan.quote_reuse.push((
                    start,
                    "nested f-string reuses the enclosing quote; Python < 3.12 cannot parse this"
                        .to_string(),
                ));
            }
            false
        } else {
            quote == outer || (outer.len() == 1 && quote.starts_with(outer))
        }
    }

    fn scan_string(&mut self, start: usize, raw: bool, in_field: Option<&str>) -> Tok {
        let quote = self.quote_here();
        let triple = quote.len() == 3;
        let reused = self.note_nested_quote(start, quote, false, in_field);
        self.pos += quote.len();
        while self.pos < self.b.len() {
            if self.at(quote) {
                self.pos += quote.len();
                break;
            }
            let byte = self.b[self.pos];
            if byte == b'\\' {
                self.check_escape(self.pos, raw);
                self.pos += 1;
                if self.pos < self.b.len() {
                    self.advance_char();
                }
                continue;
            }
            if byte == b'\n' && !triple {
                break;
            }
            self.advance_char();
        }
        if reused {
            let token = &self.src[start..self.pos];
            let outer = in_field.unwrap_or("'");
            self.scan.quote_reuse.push((
                start,
                format!(
                    "string {token} inside an f-string replacement field reuses the enclosing quote {outer}; Python < 3.12 (EasyBuild supports 3.6+, CI lints with 3.8) cannot parse this, use the other quote character"
                ),
            ));
        }
        self.scan.pieces.push(Piece {
            kind: PieceKind::Str,
            start,
            end: self.pos,
        });
        Tok {
            kind: TokKind::Str,
            start,
            end: self.pos,
        }
    }

    fn flush_literal(&mut self, start: usize, end: usize) {
        if end > start {
            self.scan.pieces.push(Piece {
                kind: PieceKind::FMiddle,
                start,
                end,
            });
        }
    }

    fn scan_fstring(&mut self, start: usize, raw: bool, in_field: Option<&str>) -> Tok {
        let quote = self.quote_here();
        let triple = quote.len() == 3;
        self.note_nested_quote(start, quote, true, in_field);
        self.pos += quote.len();
        let info = self.scan.fstrings.len();
        self.scan.fstrings.push(FStringInfo {
            start,
            has_field: false,
            nested: self.fdepth > 0,
        });
        self.fdepth += 1;
        let mut literal = self.pos;
        let mut closed = false;
        while self.pos < self.b.len() {
            if self.at(quote) {
                self.flush_literal(literal, self.pos);
                self.pos += quote.len();
                closed = true;
                break;
            }
            match self.b[self.pos] {
                b'\\' => {
                    self.check_escape(self.pos, raw);
                    self.pos += 1;
                    if self.pos < self.b.len() && !matches!(self.b[self.pos], b'{' | b'}') {
                        self.advance_char();
                    }
                }
                b'{' if self.peek(1) == Some(b'{') => self.pos += 2,
                b'}' if self.peek(1) == Some(b'}') => self.pos += 2,
                b'{' => {
                    self.flush_literal(literal, self.pos);
                    self.scan.fstrings[info].has_field = true;
                    self.pos += 1;
                    self.scan_field(quote);
                    literal = self.pos;
                }
                b'\n' if !triple => break,
                _ => self.advance_char(),
            }
        }
        if !closed {
            self.flush_literal(literal, self.pos);
        }
        self.fdepth -= 1;
        Tok {
            kind: TokKind::FStr,
            start,
            end: self.pos,
        }
    }

    fn skip_field_space(&mut self) {
        while self.pos < self.b.len() {
            match self.b[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0c | b'\\' => self.pos += 1,
                b'#' => self.skip_comment(),
                _ => break,
            }
        }
    }

    /// Scan a replacement field after its `{`, through the closing `}`.
    fn scan_field(&mut self, quote: &'a str) {
        self.skip_field_space();
        let expr_start = self.pos;
        let mut expr_end: Option<usize> = None;
        let mut level = 0usize;
        loop {
            self.skip_field_space();
            if self.pos >= self.b.len() {
                break;
            }
            let byte = self.b[self.pos];
            if level == 0 {
                if byte == b'}' {
                    expr_end.get_or_insert(self.pos);
                    self.pos += 1;
                    break;
                }
                if byte == b'!' && self.peek(1) != Some(b'=') {
                    expr_end.get_or_insert(self.pos);
                    self.pos += 1;
                    while self.pos < self.b.len() && is_ident_char(self.b[self.pos]) {
                        self.pos += 1;
                    }
                    continue;
                }
                if byte == b':' && self.peek(1) != Some(b'=') {
                    expr_end.get_or_insert(self.pos);
                    self.pos += 1;
                    self.scan_format_spec(quote);
                    break;
                }
                if expr_end.is_some() && self.at(quote) {
                    // Malformed: the f-string ends inside the field.
                    break;
                }
            }
            let tok = self.next_token(Some(quote));
            if tok.kind == TokKind::Op {
                match &self.src[tok.start..tok.end] {
                    "(" | "[" | "{" => level += 1,
                    ")" | "]" | "}" => level = level.saturating_sub(1),
                    _ => {}
                }
            }
        }
        let expr_end = expr_end.unwrap_or(self.pos).max(expr_start);
        self.scan.fields.push(Field {
            expr_start,
            expr_end,
        });
    }

    /// Scan a format spec after its `:`, through the field's closing `}`.
    fn scan_format_spec(&mut self, quote: &'a str) {
        let mut literal = self.pos;
        while self.pos < self.b.len() {
            if self.at(quote) {
                self.flush_literal(literal, self.pos);
                return;
            }
            match self.b[self.pos] {
                b'{' => {
                    self.flush_literal(literal, self.pos);
                    self.pos += 1;
                    self.scan_field(quote);
                    literal = self.pos;
                }
                b'}' => {
                    self.flush_literal(literal, self.pos);
                    self.pos += 1;
                    return;
                }
                _ => self.advance_char(),
            }
        }
        self.flush_literal(literal, self.pos);
    }
}

/// Top-level `NAME = ...` statements: name -> byte ranges of each statement.
fn top_level_assigns(text: &str, top: &[Tok]) -> BTreeMap<String, Vec<(usize, usize)>> {
    let mut out: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for line in logical_lines(top) {
        if line.len() < 2 {
            continue;
        }
        let first = line[0];
        let second = line[1];
        let at_column_zero = first.start == 0 || text.as_bytes()[first.start - 1] == b'\n';
        if first.kind == TokKind::Name
            && at_column_zero
            && second.kind == TokKind::Op
            && &text[second.start..second.end] == "="
        {
            let end = line.last().map(|tok| tok.end).unwrap_or(first.end);
            out.entry(text[first.start..first.end].to_string())
                .or_default()
                .push((first.start, end));
        }
    }
    out
}

fn logical_lines(top: &[Tok]) -> Vec<&[Tok]> {
    top.split(|tok| tok.kind == TokKind::Newline)
        .filter(|line| !line.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// CI flake8 and check-contrib pycodestyle rules

/// W291: trailing whitespace on a line that has other content (CI flake8).
/// A whitespace-only line is W293, which CI does not select.
fn trailing_whitespace(text: &str, sink: &mut Sink<'_>) {
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.trim_end_matches(['\r', '\x0c']);
        let stripped = line.trim_end_matches([' ', '\t', '\x0b']);
        if stripped.len() != line.len() && !stripped.is_empty() {
            sink.push_at(
                index + 1,
                stripped.chars().count() + 1,
                "W291",
                "trailing whitespace".into(),
            );
        }
    }
}

/// W605: invalid escape sequence in a non-raw string (CI flake8, check-contrib).
fn escape_sequences(scan: &Scan, sink: &mut Sink<'_>) {
    for &(offset, escaped) in &scan.bad_escapes {
        sink.push(
            offset,
            "W605",
            format!("invalid escape sequence '\\{escaped}'"),
        );
    }
}

const WS_NEEDED: &[&str] = &[
    "**=", "*=", "/=", "//=", "+=", "-=", "!=", "<", ">", "%=", "^=", "&=", "|=", "==", "<=", ">=",
    "<<=", ">>=", "=", "and", "in", "is", "or", "->", ":=", "@=",
];
const UNARY: &[&str] = &[">>", "**", "*", "+", "-"];
const ARITHMETIC: &[&str] = &["**", "*", "/", "//", "+", "-", "@"];
const WS_OPTIONAL_EXTRA: &[&str] = &["^", "&", "|", "<<", ">>", "%"];
const KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "print", "raise", "return", "try", "while", "with", "yield",
];

#[derive(Debug, Clone, Copy)]
enum NeedSpace {
    No,
    Yes,
    Optional { at: usize, spaced_before: bool },
}

/// E225, E226, E227, E228 and E231: pycodestyle's `missing_whitespace`, which
/// `eb --check-contrib` runs with E226 and the other default-ignored codes on.
/// f-strings are single tokens, as on the Python 3.8 CI.
fn missing_whitespace(text: &str, top: &[Tok], sink: &mut Sink<'_>) {
    let bytes = text.as_bytes();
    for line in logical_lines(top) {
        let mut need = NeedSpace::No;
        let mut brace_stack: Vec<char> = Vec::new();
        let mut prev: Option<Tok> = None;
        for &tok in line {
            let token_text = &text[tok.start..tok.end];
            let is_op = tok.kind == TokKind::Op;
            if is_op && matches!(token_text, "(" | "[" | "{") {
                brace_stack.push(token_text.chars().next().unwrap_or('('));
            } else if tok.kind == TokKind::Name && token_text == "lambda" {
                brace_stack.push('l');
            } else if is_op
                && (matches!(token_text, ")" | "]" | "}")
                    || (token_text == ":" && brace_stack.last() == Some(&'l')))
            {
                brace_stack.pop();
            }

            if is_op && matches!(token_text, "," | ";" | ":") {
                if let Some(&next) = bytes.get(tok.end) {
                    let spaced = matches!(next, b' ' | b'\t' | 0x0c | b'\r' | b'\n');
                    let slice = token_text == ":" && brace_stack.last() == Some(&'[');
                    let tuple = token_text == "," && matches!(next, b')' | b']');
                    if !spaced && !slice && !tuple {
                        sink.push(
                            tok.start,
                            "E231",
                            format!("missing whitespace after '{token_text}'"),
                        );
                    }
                }
            }

            if let (NeedSpace::Yes | NeedSpace::Optional { .. }, Some(prev_tok)) = (need, prev) {
                let prev_text = &text[prev_tok.start..prev_tok.end];
                // `/` before a closer, and `):`, are not operators that need space.
                let exempt = (prev_text == "/" && matches!(token_text, "," | ")" | ":"))
                    || (prev_text == ")" && token_text == ":");
                if tok.start != prev_tok.end {
                    if let NeedSpace::Optional {
                        at,
                        spaced_before: false,
                    } = need
                    {
                        sink.push(at, "E225", "missing whitespace around operator".into());
                    }
                } else if !exempt {
                    match need {
                        NeedSpace::Optional {
                            at,
                            spaced_before: false,
                        } => {
                            if prev_text != "**" {
                                let (code, kind) = if prev_text == "%" {
                                    ("E228", "modulo")
                                } else if ARITHMETIC.contains(&prev_text) {
                                    ("E226", "arithmetic")
                                } else {
                                    ("E227", "bitwise or shift")
                                };
                                sink.push(
                                    at,
                                    code,
                                    format!("missing whitespace around {kind} operator"),
                                );
                            }
                        }
                        _ => sink.push(
                            prev_tok.end,
                            "E225",
                            "missing whitespace around operator".into(),
                        ),
                    }
                }
                need = NeedSpace::No;
            } else if let Some(prev_tok) = prev {
                if is_op || tok.kind == TokKind::Name {
                    let prev_text = &text[prev_tok.start..prev_tok.end];
                    // `foo(bar=1)` and `lambda x=1:` are not missing whitespace.
                    let keyword_default =
                        token_text == "=" && matches!(brace_stack.last(), Some('l' | '('));
                    let mut optional = false;
                    if !keyword_default {
                        if WS_NEEDED.contains(&token_text) {
                            need = NeedSpace::Yes;
                        } else if UNARY.contains(&token_text) {
                            optional = (prev_tok.kind == TokKind::Op
                                && matches!(prev_text, "}" | "]" | ")"))
                                || (prev_tok.kind != TokKind::Op && !KEYWORDS.contains(&prev_text));
                        } else if is_op
                            && (ARITHMETIC.contains(&token_text)
                                || WS_OPTIONAL_EXTRA.contains(&token_text))
                        {
                            optional = true;
                        }
                    }
                    if optional {
                        need = NeedSpace::Optional {
                            at: prev_tok.end,
                            spaced_before: tok.start != prev_tok.end,
                        };
                    } else if matches!(need, NeedSpace::Yes) && tok.start == prev_tok.end {
                        sink.push(
                            prev_tok.end,
                            "E225",
                            "missing whitespace around operator".into(),
                        );
                        need = NeedSpace::No;
                    }
                }
            }
            prev = Some(tok);
        }
    }
}

const F541_MESSAGE: &str = "f-string is missing placeholders. Drop the f prefix when the string only contains EasyBuild templates such as %(name)s, %(version)s, or %(builddir)s. An f-string needs a Python placeholder, for example f'lib/libfoo.{SHLIB_EXT}'";

fn flush_fstring_group(
    group: &mut Vec<Tok>,
    by_start: &HashMap<usize, FStringInfo>,
    sink: &mut Sink<'_>,
) {
    let fstrings: Vec<&FStringInfo> = group
        .iter()
        .filter(|tok| tok.kind == TokKind::FStr)
        .filter_map(|tok| by_start.get(&tok.start))
        .collect();
    if !fstrings.is_empty() && fstrings.iter().all(|info| !info.has_field) {
        sink.push(group[0].start, "F541", F541_MESSAGE.into());
    }
    group.clear();
}

/// F541: f-string without a placeholder (CI flake8). Implicitly concatenated
/// pieces count as one string, as in Python's AST.
fn fstring_placeholders(scan: &Scan, sink: &mut Sink<'_>) {
    let by_start: HashMap<usize, FStringInfo> = scan
        .fstrings
        .iter()
        .map(|info| (info.start, *info))
        .collect();
    let mut group: Vec<Tok> = Vec::new();
    for &tok in &scan.top {
        if matches!(tok.kind, TokKind::Str | TokKind::FStr) {
            group.push(tok);
        } else if !group.is_empty() {
            flush_fstring_group(&mut group, &by_start, sink);
        }
    }
    if !group.is_empty() {
        flush_fstring_group(&mut group, &by_start, sink);
    }
    for info in scan.fstrings.iter().filter(|info| info.nested) {
        if !info.has_field {
            sink.push(info.start, "F541", F541_MESSAGE.into());
        }
    }
}

// ---------------------------------------------------------------------------
// EB_* rules on tokens and lines

fn quote_reuse(scan: &Scan, sink: &mut Sink<'_>) {
    for (offset, message) in &scan.quote_reuse {
        sink.push(*offset, "EB_FSTRING_QUOTE_REUSE", message.clone());
    }
}

fn piece_text<'t>(text: &'t str, piece: &Piece) -> &'t str {
    &text[piece.start..piece.end]
}

/// EB_FMT_MISSING_F: a plain string with `{NAME}` for a name the recipe or
/// EasyBuild defines, typically the second half of a split f-string.
fn missing_fprefix(text: &str, scan: &Scan, names: &HashSet<&str>, sink: &mut Sink<'_>) {
    for piece in scan
        .pieces
        .iter()
        .filter(|piece| piece.kind == PieceKind::Str)
    {
        let body = piece_text(text, piece);
        let bytes = body.as_bytes();
        for open in 0..bytes.len() {
            if bytes[open] != b'{' || (open > 0 && matches!(bytes[open - 1], b'$' | b'{')) {
                continue;
            }
            let mut end = open + 1;
            if end >= bytes.len() || !(bytes[end].is_ascii_alphabetic() || bytes[end] == b'_') {
                continue;
            }
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if bytes.get(end) != Some(&b'}') || bytes.get(end + 1) == Some(&b'}') {
                continue;
            }
            let ident = &body[open + 1..end];
            if names.contains(ident) {
                sink.push(
                    piece.start,
                    "EB_FMT_MISSING_F",
                    format!("string contains {{{ident}}} but is not an f-string, so it stays literal; add the f prefix"),
                );
                break;
            }
        }
    }
}

static DERIVED_NAMELOWER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^name\.lower\(\)(\[[^\]]*\])*$").expect("namelower regex"));
static DERIVED_LETTERLOWER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^nameletter\.lower\(\)(\[[^\]]*\])*$").expect("nameletterlower regex")
});
static DERIVED_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^version(\.split\([^)]*\))?(\[[^\]]*\])*$").expect("version regex")
});

/// EB_FMT_TEMPLATE_VAR: an f-string field that computes what a template
/// already provides (`{name.lower()}` is `%(namelower)s`), or a plain
/// `{name}` / `{version}` / `{versionsuffix}` inside `exts_list` or
/// `components`, where the template resolves per extension and the Python
/// name is the parent's. A plain top-level `{name}` / `{version}` is not
/// flagged, since merged easyconfigs use it, and `%(name)s` text inside an
/// f-string (`f'lib/%(name)s.{SHLIB_EXT}'`) is the right form.
fn template_vars(
    text: &str,
    scan: &Scan,
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
    sink: &mut Sink<'_>,
) {
    let ext_ranges: Vec<(usize, usize)> = ["exts_list", "components"]
        .iter()
        .filter_map(|key| assigns.get(*key))
        .flatten()
        .copied()
        .collect();
    for field in &scan.fields {
        let compact: String = text[field.expr_start..field.expr_end]
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        let expr = compact.trim_end_matches('=');
        let template = if DERIVED_NAMELOWER.is_match(expr) {
            Some("namelower")
        } else if DERIVED_LETTERLOWER.is_match(expr) {
            Some("nameletterlower")
        } else if expr != "version" && DERIVED_VERSION.is_match(expr) {
            Some("version_major")
        } else {
            None
        };
        if let Some(template) = template {
            sink.push(
                field.expr_start,
                "EB_FMT_TEMPLATE_VAR",
                format!("f-string computes `{expr}`; use the EasyBuild template %({template})s instead (templates first, f-strings only for values without a template)"),
            );
            continue;
        }
        let in_ext = ext_ranges
            .iter()
            .any(|(start, end)| *start <= field.expr_start && field.expr_start < *end);
        if in_ext && matches!(expr, "name" | "version" | "versionsuffix") {
            sink.push(
                field.expr_start,
                "EB_FMT_TEMPLATE_VAR",
                format!("{{{expr}}} in exts_list/components is the parent's {expr}; use %({expr})s, which EasyBuild resolves per extension/component"),
            );
        }
    }
}

/// EB_SHLIB_HARDCODED: `.so` at the end of a word in a string.
fn hardcoded_shlib(text: &str, scan: &Scan, sink: &mut Sink<'_>) {
    for piece in &scan.pieces {
        let body = piece_text(text, piece);
        let hit = body.match_indices(".so").any(|(index, _)| {
            body[index + 3..]
                .chars()
                .next()
                .is_none_or(|next| next != '.' && next != '_' && !next.is_alphanumeric())
        });
        if hit {
            sink.push(
                piece.start,
                "EB_SHLIB_HARDCODED",
                "hardcoded '.so'; use SHLIB_EXT in an f-string (e.g. f'lib/libfoo.{SHLIB_EXT}')"
                    .into(),
            );
        }
    }
}

static TEMPLATE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%\((\w+)\)s").expect("template regex"));

/// EB_TEMPLATE_UNKNOWN: `%(x)s` that EasyBuild does not define.
fn unknown_templates(text: &str, scan: &Scan, sink: &mut Sink<'_>) {
    let mut known: Vec<&str> = vocab::TEMPLATE_NAMES
        .iter()
        .chain(vocab::TEMPLATE_ALIASES.iter())
        .copied()
        .collect();
    known.sort_unstable();
    known.dedup();
    for piece in &scan.pieces {
        for captures in TEMPLATE_REF.captures_iter(piece_text(text, piece)) {
            let name = &captures[1];
            if known.binary_search(&name).is_ok() {
                continue;
            }
            let hint = close_match(&name.to_lowercase(), &known)
                .map(|suggestion| format!(" (did you mean %({suggestion})s?)"))
                .unwrap_or_default();
            sink.push(
                piece.start,
                "EB_TEMPLATE_UNKNOWN",
                format!("unknown EasyBuild template %({name})s{hint}"),
            );
        }
    }
}

/// `difflib.get_close_matches(word, candidates, n=1, cutoff=0.6)`.
fn close_match<'c>(word: &str, candidates: &[&'c str]) -> Option<&'c str> {
    let mut best: Option<(f64, &'c str)> = None;
    for &candidate in candidates {
        let score = sequence_ratio(candidate, word);
        if score < 0.6 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_score, best_word)) => {
                score > best_score || (score == best_score && candidate > best_word)
            }
        };
        if better {
            best = Some((score, candidate));
        }
    }
    best.map(|(_, word)| word)
}

/// `difflib.SequenceMatcher(None, a, b).ratio()` for short strings (no junk).
fn sequence_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches = matching_chars(&a, &b, (0, a.len()), (0, b.len()));
    2.0 * matches as f64 / total as f64
}

/// Total size of difflib's matching blocks between `a[alo..ahi]` and `b[blo..bhi]`.
fn matching_chars(
    a: &[char],
    b: &[char],
    (alo, ahi): (usize, usize),
    (blo, bhi): (usize, usize),
) -> usize {
    let (mut best_i, mut best_j, mut best_size) = (alo, blo, 0usize);
    let mut lengths: HashMap<usize, usize> = HashMap::new();
    for (offset_i, &a_ch) in a[alo..ahi].iter().enumerate() {
        let i = alo + offset_i;
        let mut next: HashMap<usize, usize> = HashMap::new();
        for (offset_j, &b_ch) in b[blo..bhi].iter().enumerate() {
            let j = blo + offset_j;
            if a_ch != b_ch {
                continue;
            }
            let k = j
                .checked_sub(1)
                .and_then(|previous| lengths.get(&previous))
                .copied()
                .unwrap_or(0)
                + 1;
            next.insert(j, k);
            if k > best_size {
                best_i = i + 1 - k;
                best_j = j + 1 - k;
                best_size = k;
            }
        }
        lengths = next;
    }
    if best_size == 0 {
        return 0;
    }
    best_size
        + matching_chars(a, b, (alo, best_i), (blo, best_j))
        + matching_chars(a, b, (best_i + best_size, ahi), (best_j + best_size, bhi))
}

static EASYBLOCK_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^easyblock\s*=\s*['"]([^'"]+)['"]"#).expect("easyblock regex"));

fn easyblock_name(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| EASYBLOCK_LINE.captures(line))
        .next_back()
        .map(|captures| captures[1].to_string())
}

static PYTHON_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bpython (-c|-m|cc|[^ ]*\w+.py) ").expect("python call regex"));
static PYTHON_NO_USER_SITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bpython (-\w+ )*-s ").expect("python -s regex"));

/// EB_PY_NO_USER_SITE: port of easyconfigs `test_pr_python_packages`.
fn python_user_site(text: &str, easyblock: Option<&str>, file_name: &str, sink: &mut Sink<'_>) {
    if easyblock.is_some_and(|block| vocab::PYTHON_EASYBLOCKS.contains(&block)) {
        return;
    }
    for (index, line) in text.lines().enumerate() {
        let code = match line.find("# ") {
            Some(cut) => &line[..cut],
            None => line,
        };
        if PYTHON_CALL.is_match(code) && !PYTHON_NO_USER_SITE.is_match(code) {
            sink.push_at(
                index + 1,
                1,
                "EB_PY_NO_USER_SITE",
                format!(
                    "Python invocation in '{code}' (line #{}) should use the '-s' parameter in {file_name}",
                    index + 1
                ),
            );
        }
    }
}

static PARAM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+)\s*=").expect("param line regex"));

/// EB_REDEFINED_PARAM: `^(\w+)\s*=` seen twice, as the test suite checks it.
fn redefined_params(text: &str, sink: &mut Sink<'_>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some(captures) = PARAM_LINE.captures(line) else {
            continue;
        };
        let key = captures[1].to_string();
        if let Some(first) = seen.get(&key) {
            sink.push_at(
                index + 1,
                1,
                "EB_REDEFINED_PARAM",
                format!("easyconfig parameter '{key}' is defined again (first on line {first}); define each parameter once"),
            );
        } else {
            seen.insert(key, index + 1);
        }
    }
}

/// EB_LOCAL_VAR_NAMING: a top-level name that is neither a generic parameter
/// nor an extra option of the easyblock. Skipped when the easyblock's
/// options are unknown (anything but a generic easyblock).
fn local_var_naming(
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
    easyblock: Option<&str>,
    sink: &mut Sink<'_>,
) {
    let Some(block) = easyblock else {
        return;
    };
    let Some((_, extras)) = vocab::GENERIC_EASYBLOCK_EXTRAS
        .iter()
        .find(|(name, _)| *name == block)
    else {
        return;
    };
    for (name, statements) in assigns {
        let known =
            vocab::DEFAULT_PARAMS.contains(&name.as_str()) || extras.contains(&name.as_str());
        let local = name.starts_with("local_") || name.starts_with('_');
        if known || local || name.chars().count() <= 1 {
            continue;
        }
        sink.push(
            statements[0].0,
            "EB_LOCAL_VAR_NAMING",
            format!("'{name}' is not a generic easyconfig parameter; if it is a local variable rename it to 'local_{name}' (unless it is an extra option of the easyblock in use)"),
        );
    }
}

/// EB_FMT_TEMPLATE_REMOVED (diff mode): a template the base had was replaced
/// by a Python expression.
fn templates_removed(text: &str, base: &str, sink: &mut Sink<'_>) {
    let count = |source: &str| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for captures in TEMPLATE_REF.captures_iter(source) {
            *counts.entry(captures[1].to_string()).or_default() += 1;
        }
        counts
    };
    let before = count(base);
    let after = count(text);
    let patterns: [(&str, &str); 5] = [
        ("name", r"\{name\}"),
        ("namelower", r"\{name\.lower\(\)\}"),
        ("version", r"\{version\}"),
        ("version_major_minor", r"\{.*version.*\}"),
        ("versionsuffix", r"\{versionsuffix\}"),
    ];
    for (template, had) in &before {
        if after.get(template).copied().unwrap_or(0) >= *had {
            continue;
        }
        let Some((_, pattern)) = patterns.iter().find(|(name, _)| *name == template.as_str())
        else {
            continue;
        };
        let regex = Regex::new(pattern).expect("template expression regex");
        for (index, line) in text.lines().enumerate() {
            if regex.is_match(line) && !line.trim_start().starts_with('#') {
                sink.push_at(
                    index + 1,
                    1,
                    "EB_FMT_TEMPLATE_REMOVED",
                    format!("%({template})s template from the base version was replaced by a Python expression; keep the template"),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EB_* rules on the AST

fn ast_rules(text: &str, suite: &[ast::Stmt], file_name: &str, sink: &mut Sink<'_>) {
    let mut visitor = FormatVisitor {
        found: Vec::new(),
        text,
    };
    for statement in suite.iter().cloned() {
        visitor.visit_stmt(statement);
    }
    for (offset, code, message) in visitor.found {
        sink.push(offset, code, message);
    }

    let mut assigns: HashMap<&str, Vec<&ast::StmtAssign>> = HashMap::new();
    for statement in suite {
        if let ast::Stmt::Assign(assign) = statement {
            for target in &assign.targets {
                if let ast::Expr::Name(name) = target {
                    assigns.entry(name.id.as_str()).or_default().push(assign);
                }
            }
        }
    }
    sanity_paths_shape(&assigns, sink);
    duplicate_dependencies(&assigns, sink);
    checksum_count(&assigns, file_name, sink);
}

struct FormatVisitor<'t> {
    found: Vec<(usize, &'static str, String)>,
    text: &'t str,
}

impl FormatVisitor<'_> {
    fn note_binop(&mut self, binop: &ast::ExprBinOp) {
        let offset = u32::from(binop.range.start()) as usize;
        match binop.op {
            ast::Operator::Mod => {
                let ast::Expr::Constant(constant) = binop.left.as_ref() else {
                    return;
                };
                let ast::Constant::Str(value) = &constant.value else {
                    return;
                };
                let hint = if value.contains("%%(") {
                    "; un-escape %%(...)s to %(...)s when converting"
                } else {
                    ""
                };
                let guidance = super::review::percent_guidance(&binop.right);
                let fragment = super::review::source_fragment(self.text, binop.range);
                self.found.push((
                    offset,
                    "EB_FMT_PERCENT",
                    format!("%-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{{SHLIB_EXT}}') and keep %(...)s templates as they are{hint}. `{fragment}`: {guidance}"),
                ));
            }
            ast::Operator::Add => {
                let sides = [binop.left.as_ref(), binop.right.as_ref()];
                let literal = sides
                    .iter()
                    .any(|side| matches!(side, ast::Expr::Constant(_) | ast::Expr::JoinedStr(_)));
                let constant = sides.iter().any(|side| {
                    matches!(side, ast::Expr::Name(name) if matches!(name.id.as_str(), "SHLIB_EXT" | "ARCH"))
                });
                if !(literal && constant) {
                    return;
                }
                let name = sides
                    .iter()
                    .find_map(|side| match side {
                        ast::Expr::Name(name) => Some(name.id.as_str().to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();
                self.found.push((
                    offset,
                    "EB_FMT_CONCAT",
                    format!("string concatenation with {name}; put it inside the f-string instead (e.g. f'lib/libfoo.{{{name}}}'). Templates stay as they are: f'lib/%(name)s.{{{name}}}' is the right form"),
                ));
            }
            _ => {}
        }
    }
}

impl Visitor for FormatVisitor<'_> {
    fn visit_expr(&mut self, node: ast::Expr) {
        if let ast::Expr::BinOp(binop) = &node {
            self.note_binop(binop);
        }
        self.generic_visit_expr(node);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListShape {
    /// A list; `Some(true)` when it is known to be non-empty.
    List(Option<bool>),
    NotList,
    Unknown,
}

fn list_shape(expr: &ast::Expr) -> ListShape {
    match expr {
        ast::Expr::List(list) => ListShape::List(Some(!list.elts.is_empty())),
        ast::Expr::ListComp(_) => ListShape::List(None),
        ast::Expr::BinOp(binop) if matches!(binop.op, ast::Operator::Add) => {
            match (list_shape(&binop.left), list_shape(&binop.right)) {
                (ListShape::List(left), ListShape::List(right)) => {
                    ListShape::List(match (left, right) {
                        (Some(true), _) | (_, Some(true)) => Some(true),
                        (Some(false), Some(false)) => Some(false),
                        _ => None,
                    })
                }
                (ListShape::NotList, _) | (_, ListShape::NotList) => ListShape::NotList,
                _ => ListShape::Unknown,
            }
        }
        ast::Expr::Constant(_)
        | ast::Expr::Tuple(_)
        | ast::Expr::Dict(_)
        | ast::Expr::Set(_)
        | ast::Expr::JoinedStr(_) => ListShape::NotList,
        _ => ListShape::Unknown,
    }
}

/// `Some(true)` good shape, `Some(false)` bad, `None` when it cannot be told
/// without evaluating the recipe.
fn sanity_shape_ok(expr: &ast::Expr) -> Option<bool> {
    let ast::Expr::Dict(dict) = expr else {
        return match expr {
            ast::Expr::Constant(_)
            | ast::Expr::List(_)
            | ast::Expr::Tuple(_)
            | ast::Expr::Set(_)
            | ast::Expr::JoinedStr(_) => Some(false),
            _ => None,
        };
    };
    let mut keys = Vec::new();
    for key in &dict.keys {
        match key {
            Some(ast::Expr::Constant(constant)) => match &constant.value {
                ast::Constant::Str(value) => keys.push(value.clone()),
                _ => return Some(false),
            },
            _ => return None,
        }
    }
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    if unique != ["dirs", "files"] {
        return Some(false);
    }
    // Python keeps the last value of a repeated key.
    let mut values: HashMap<&str, &ast::Expr> = HashMap::new();
    for (key, value) in keys.iter().zip(dict.values.iter()) {
        values.insert(key.as_str(), value);
    }
    let mut any_nonempty = false;
    let mut unknown = false;
    for value in values.values() {
        match list_shape(value) {
            ListShape::NotList => return Some(false),
            ListShape::Unknown => unknown = true,
            ListShape::List(Some(true)) | ListShape::List(None) => any_nonempty = true,
            ListShape::List(Some(false)) => {}
        }
    }
    if unknown && !any_nonempty {
        return None;
    }
    Some(any_nonempty)
}

/// EB_SANITY_PATHS_SHAPE: port of the easyconfigs `test_sanity_check_paths`.
fn sanity_paths_shape(assigns: &HashMap<&str, Vec<&ast::StmtAssign>>, sink: &mut Sink<'_>) {
    let Some(statements) = assigns.get("sanity_check_paths") else {
        return;
    };
    let Some(last) = statements.last() else {
        return;
    };
    if sanity_shape_ok(&last.value) != Some(false) {
        return;
    }
    for statement in statements {
        sink.push(
            u32::from(statement.range.start()) as usize,
            "EB_SANITY_PATHS_SHAPE",
            "sanity_check_paths must have exactly the keys 'files' and 'dirs', both lists, and at least one of them non-empty".into(),
        );
    }
}

fn py_str_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr` of a literal tuple or constant, or `None` when the value is
/// not a plain hashable literal.
fn literal_repr(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Str(value) => Some(py_str_repr(value)),
            ast::Constant::Int(value) => Some(value.to_string()),
            ast::Constant::Bool(value) => Some(if *value { "True" } else { "False" }.to_string()),
            ast::Constant::None => Some("None".to_string()),
            _ => None,
        },
        ast::Expr::Tuple(tuple) => {
            let items = tuple
                .elts
                .iter()
                .map(literal_repr)
                .collect::<Option<Vec<_>>>()?;
            Some(match items.len() {
                1 => format!("({},)", items[0]),
                _ => format!("({})", items.join(", ")),
            })
        }
        _ => None,
    }
}

/// EB_DEP_DUPLICATE: the same tuple twice in (build)dependencies.
fn duplicate_dependencies(assigns: &HashMap<&str, Vec<&ast::StmtAssign>>, sink: &mut Sink<'_>) {
    for key in ["dependencies", "builddependencies"] {
        for statement in assigns.get(key).into_iter().flatten() {
            let ast::Expr::List(list) = statement.value.as_ref() else {
                continue;
            };
            let mut seen: HashMap<String, usize> = HashMap::new();
            for element in &list.elts {
                let Some(literal) = literal_repr(element) else {
                    continue;
                };
                let offset = u32::from(element.range().start()) as usize;
                if let Some(first) = seen.get(&literal) {
                    sink.push(
                        offset,
                        "EB_DEP_DUPLICATE",
                        format!("duplicate entry {literal} in {key} (first on line {first})"),
                    );
                } else {
                    let line = sink.index.line(offset);
                    seen.insert(literal, line);
                }
            }
        }
    }
}

/// EB_CHECKSUM_COUNT: port of `EasyBlock.check_checksums_for` (top level only).
fn checksum_count(
    assigns: &HashMap<&str, Vec<&ast::StmtAssign>>,
    file_name: &str,
    sink: &mut Sink<'_>,
) {
    let count = |key: &str| -> usize {
        match assigns.get(key).and_then(|statements| statements.last()) {
            None => 0,
            Some(statement) => match statement.value.as_ref() {
                ast::Expr::List(list) => list.elts.len(),
                ast::Expr::Tuple(tuple) => tuple.elts.len(),
                _ => 1,
            },
        }
    };
    let sources = count("sources") + count("data_sources");
    let patches = count("patches");
    let checksums = count("checksums");
    if sources + patches == 0 || checksums == 0 || sources + patches == checksums {
        return;
    }
    let Some(last) = assigns
        .get("checksums")
        .and_then(|statements| statements.last())
    else {
        return;
    };
    sink.push(
        u32::from(last.range.start()) as usize,
        "EB_CHECKSUM_COUNT",
        format!("Checksums missing for one or more sources/patches in {file_name}: found {sources} sources + {patches} patches vs {checksums} checksums"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(text: &str) -> Vec<StyleFinding> {
        let mut out = Vec::new();
        extend_rules(text, &LintOptions::default(), &mut out);
        out
    }

    fn codes(findings: &[StyleFinding]) -> Vec<(usize, String)> {
        let mut out: Vec<_> = findings
            .iter()
            .map(|finding| (finding.line, finding.code.clone()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn mixed_template_and_constant_fstring_is_clean() {
        let findings = lint(
            "easyblock = 'ConfigureMake'\nsanity_check_paths = {\n    'files': [f'lib/%(name)s.{SHLIB_EXT}'],\n    'dirs': [],\n}\n",
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn concatenating_shlib_ext_is_flagged() {
        let findings = lint("files = ['lib/%(name)s.' + SHLIB_EXT]\n");
        assert_eq!(codes(&findings), [(1, "EB_FMT_CONCAT".to_string())]);
        assert!(findings[0].message.contains("f'lib/%(name)s.{SHLIB_EXT}'"));
    }

    #[test]
    fn syntax_error_is_reported() {
        let findings = lint("sources = ['f'libexec/7z.{SHLIB_EXT}']\n");
        assert!(
            findings.iter().any(|finding| finding.code == "E999"),
            "{findings:?}"
        );
        assert!(python_syntax_error("x = 1\n").is_none());
        assert!(python_syntax_error("x = (\n").is_some());
    }

    #[test]
    fn plain_top_level_name_and_version_are_tolerated() {
        let findings = lint("sources = [f'{name}-{version}.tar.gz']\n");
        assert!(findings.is_empty(), "{findings:?}");
        let ext = lint(
            "exts_list = [\n    ('foo', '1.0', {'source_tmpl': f'{name}-{version}.tar.gz'}),\n]\n",
        );
        assert_eq!(
            codes(&ext),
            [
                (2, "EB_FMT_TEMPLATE_VAR".to_string()),
                (2, "EB_FMT_TEMPLATE_VAR".to_string())
            ]
        );
    }

    #[test]
    fn derived_values_point_at_templates() {
        let findings = lint(
            "a = f'{name.lower()}'\nb = f'{version.split(\".\")[0]}'\nc = f'{nameletter.lower()}'\n",
        );
        let messages: Vec<_> = findings
            .iter()
            .map(|finding| finding.message.as_str())
            .collect();
        assert_eq!(messages.len(), 3, "{findings:?}");
        assert!(messages[0].contains("%(namelower)s"), "{messages:?}");
        assert!(messages[1].contains("%(version_major)s"), "{messages:?}");
        assert!(messages[2].contains("%(nameletterlower)s"), "{messages:?}");
    }

    #[test]
    fn whitespace_rules_follow_pycodestyle() {
        let findings = lint(
            "toolchain = {'name':'GCC'}\nx = 2*1\ny = -1\nz = f(*args, **kw)\nw = v[1:2]\nt = (3,)\nu = 2**3\ns = 'a %s' % 'b'\n",
        );
        assert_eq!(
            codes(&findings),
            [
                (1, "E231".to_string()),
                (2, "E226".to_string()),
                (8, "EB_FMT_PERCENT".to_string())
            ]
        );
    }

    #[test]
    fn quote_reuse_and_escapes() {
        let found = codes(&lint(
            "a = f'{'_'.join(v)}'\nb = 'x\\$y'\nc = r'x\\$y'\nd = 'x\\\\$y'\n",
        ));
        assert!(
            found.contains(&(1, "EB_FSTRING_QUOTE_REUSE".to_string())),
            "{found:?}"
        );
        assert!(found.contains(&(2, "W605".to_string())), "{found:?}");
        assert!(!found.contains(&(3, "W605".to_string())), "{found:?}");
        assert!(!found.contains(&(4, "W605".to_string())), "{found:?}");
        let other = codes(&lint("a = f'{\"_\".join(v)}'\n"));
        assert!(other.is_empty(), "{other:?}");
    }

    #[test]
    fn difflib_suggestion_matches_python() {
        let known = ["cuda_cc_cmake", "cuda_cc_nvhpc", "version"];
        assert_eq!(close_match("cuda__cc_cmake", &known), Some("cuda_cc_cmake"));
        assert_eq!(close_match("zzz", &known), None);
        assert!((sequence_ratio("abcd", "bcde") - 0.75).abs() < 1e-9);
    }

    #[test]
    fn f541_counts_implicit_concatenation_as_one_string() {
        assert!(lint("x = (f'a{SHLIB_EXT}' 'b')\n").is_empty());
        assert_eq!(codes(&lint("x = (f'a' 'b')\n")), [(1, "F541".to_string())]);
    }
}
