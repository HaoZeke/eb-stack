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
    /// The version the change started from. Turns on the diff-mode rules:
    /// EB_FMT_TEMPLATE_REMOVED, EB_SANITY_ENTRY_REMOVED,
    /// EB_SANITY_PATHS_REMOVED and EB_EASYBLOCK_CHANGED.
    pub base: Option<&'a str>,
    /// Scope `EB_FMT_*` findings to lines that differ from `base`: findings
    /// on unchanged lines become `info`. Set when the caller passed the base
    /// explicitly. Without it (or without a base), only untouched
    /// `%%(...)s`-escaped `%` entries are `info`.
    pub scope_to_base: bool,
    /// Lines (1-based, in the linted file) the review is about. `EB_FMT_*`
    /// findings there always count against clean.
    pub review_lines: &'a [usize],
    /// The caller passed `review_scope: "file"`: the review is about the
    /// whole file, so a review-convention warning such as
    /// `EB_SHLIB_HARDCODED` counts on every line, not only on `review_lines`.
    pub file_scope: bool,
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
    fstring_escaped_templates(text, &scan, &mut sink);
    quote_reuse(&scan, &mut sink);
    missing_fprefix(text, &scan, &names, &mut sink);
    template_vars(text, &scan, &assigns, &mut sink);
    hardcoded_shlib(text, &scan, &assigns, &mut sink);
    sanity_command_dirs(text, &scan, &assigns, &mut sink);
    unknown_templates(text, &scan, &assigns, &mut sink);
    let easyblock = easyblock_name(text);
    python_user_site(text, easyblock.as_deref(), file_name, &mut sink);
    redefined_params(text, &mut sink);
    local_var_naming(&assigns, easyblock.as_deref(), &mut sink);
    if let Some(base) = options.base {
        templates_removed(text, base, &mut sink);
    }

    match ast::Suite::parse(text, "<easyconfig>") {
        Ok(suite) => {
            ast_rules(text, &suite, file_name, &mut sink);
            if let Some(base) = options.base {
                if let Ok(base_suite) = ast::Suite::parse(base, "<base>") {
                    diff_rules(
                        text,
                        &suite,
                        base,
                        &base_suite,
                        options.review_lines,
                        &mut sink,
                    );
                }
            }
        }
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
            info: false,
            in_review: false,
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

/// EB_FSTRING_ESCAPED_TEMPLATE: `%%(...)s` in the literal part of an
/// f-string. EasyBuild resolves templates with `%`, so `%%` stays a literal
/// `%` and the template is never filled in (`lib/libtcl%(version_major)s.so`
/// ends up in the path). Inside an f-string the template is plain `%(...)s`.
fn fstring_escaped_templates(text: &str, scan: &Scan, sink: &mut Sink<'_>) {
    for piece in &scan.pieces {
        if piece.kind != PieceKind::FMiddle {
            continue;
        }
        let body = piece_text(text, piece);
        if let Some(at) = body.find("%%(") {
            sink.push(
                piece.start + at,
                "EB_FSTRING_ESCAPED_TEMPLATE",
                "%%(...)s inside an f-string is not a template: EasyBuild leaves a literal %(...)s in the value. The %% escape was only needed with the % operator; write %(...)s in the f-string".into(),
            );
        }
    }
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

/// EB_SHLIB_HARDCODED: `.so` at the end of a word in a string, when the
/// word is a library path (`lib/` or `lib64/` in it) or the string is part of
/// `sanity_check_paths`. Build-tree names such as `'bin/7z.so'` in
/// `files_to_copy` are the file's real name and are left alone.
fn hardcoded_shlib(
    text: &str,
    scan: &Scan,
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
    sink: &mut Sink<'_>,
) {
    let sanity = assigns
        .get("sanity_check_paths")
        .map(Vec::as_slice)
        .unwrap_or_default();
    for piece in &scan.pieces {
        let body = piece_text(text, piece);
        let in_sanity = sanity
            .iter()
            .any(|&(start, end)| piece.start >= start && piece.end <= end);
        let hit = body.match_indices(".so").any(|(index, _)| {
            let word_end = body[index + 3..]
                .chars()
                .next()
                .is_none_or(|next| next != '.' && next != '_' && !next.is_alphanumeric());
            if !word_end {
                return false;
            }
            let word_start = body[..index]
                .rfind(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ','))
                .map_or(0, |at| at + 1);
            let word = &body[word_start..index];
            in_sanity || shared_library_path(word)
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

static CD_INSTALLDIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bcd\s+["']?(%\(installdir\)s|\$\{?EBROOT\w+\}?)"#).expect("cd regex")
});

/// EB_SANITY_CMD_BUILDDIR and EB_SANITY_CMD_CD_INSTALLDIR: where a
/// `sanity_check_commands` entry runs.
///
/// EasyBuild runs sanity check commands after loading the generated module,
/// from a fresh empty temporary directory (`eb-sanity-check-*`), because
/// "using the build or installation directory can produce false positives and
/// polute them with files" (framework `EasyBlock._sanity_check_step`). With
/// `--sanity-check-only` or `--module-only` every other step is skipped, so
/// `%(builddir)s` and `%(start_dir)s` do not exist, and after a normal build
/// the build directory is cleaned up. A command that `cd`s into the
/// installation directory to run something writes its output there. Both
/// are warnings; on a review line or a line you edited they block clean.
fn sanity_command_dirs(
    text: &str,
    scan: &Scan,
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
    sink: &mut Sink<'_>,
) {
    let Some(statements) = assigns.get("sanity_check_commands") else {
        return;
    };
    for &(start, end) in statements {
        let uses_tmp = text[start..end].contains("mktemp");
        for piece in scan
            .pieces
            .iter()
            .filter(|piece| piece.start >= start && piece.end <= end)
        {
            let body = piece_text(text, piece);
            if body.contains("%(builddir)s") || body.contains("%(start_dir)s") {
                sink.push(
                    piece.start,
                    "EB_SANITY_CMD_BUILDDIR",
                    "sanity_check_commands must not depend on the build directory: it does not exist with eb --sanity-check-only or --module-only and is removed after a build. EasyBuild runs each command from an empty temporary directory after loading the module; copy what the command needs from %(installdir)s into a fresh directory (cd $(mktemp -d) && cp -r %(installdir)s/... .) and run it there".into(),
                );
            } else if !uses_tmp && CD_INSTALLDIR.is_match(body) {
                sink.push(
                    piece.start,
                    "EB_SANITY_CMD_CD_INSTALLDIR",
                    "a sanity check command that cds into the installation directory writes its output there (and the installation may be read-only); run it from a fresh temporary directory instead: cd $(mktemp -d) && cp -r %(installdir)s/... . && <command>".into(),
                );
            }
        }
    }
}

/// True for a path whose directory part is a library directory.
fn shared_library_path(word: &str) -> bool {
    word.starts_with("lib/")
        || word.starts_with("lib64/")
        || word.contains("/lib/")
        || word.contains("/lib64/")
}

static TEMPLATE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%\((\w+)\)s").expect("template regex"));

/// EB_TEMPLATE_UNKNOWN: `%(x)s` that EasyBuild does not define.
fn unknown_templates(
    text: &str,
    scan: &Scan,
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
    sink: &mut Sink<'_>,
) {
    let mut known: Vec<&str> = vocab::TEMPLATE_NAMES
        .iter()
        .chain(vocab::TEMPLATE_ALIASES.iter())
        .chain(vocab::EASYBLOCK_TEMPLATES.iter())
        .copied()
        .collect();
    known.sort_unstable();
    known.dedup();
    let local_formats = percent_formatted_spans(text, assigns);
    for piece in &scan.pieces {
        if percent_formatted_literal(text, piece)
            || local_formats
                .iter()
                .any(|&(start, end)| piece.start >= start && piece.end <= end)
        {
            // `'%(key)s' % {...}` and `local_x = '%(key)s'` used as
            // `local_x % {...}`: the keys belong to Python's own formatting.
            continue;
        }
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

/// True when a plain string literal is the left operand of `%`.
fn percent_formatted_literal(text: &str, piece: &Piece) -> bool {
    if piece.kind != PieceKind::Str {
        return false;
    }
    let rest = text[piece.end..].trim_start_matches(['\'', '"']);
    let rest = rest.trim_start_matches([' ', '\t']);
    rest.starts_with('%') && !rest.starts_with("%=")
}

/// Spans of top-level assignments whose name is later used as `NAME % ...`.
fn percent_formatted_spans(
    text: &str,
    assigns: &BTreeMap<String, Vec<(usize, usize)>>,
) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for (name, places) in assigns {
        let Ok(used) = Regex::new(&format!(r"\b{}\s*%[^=]", regex::escape(name))) else {
            continue;
        };
        if used.is_match(text) {
            spans.extend(places.iter().copied());
        }
    }
    spans
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
    empty_dependency_versions(&assigns, sink);
    checksum_count(&assigns, file_name, sink);
    trailing_comma_tuples(text, suite, sink);
}

/// EB_PARAM_TUPLE_COMMA: `name = [...],` or `name = {...},` at top level. The
/// trailing comma makes the value a one-element tuple that holds the list or
/// dict, which EasyBuild then rejects or misreads (proof v7: an edit that
/// removed the line after a closing `}` left `sanity_check_paths = {...},`).
/// A parenthesised tuple is written on purpose and is left alone.
fn trailing_comma_tuples(text: &str, suite: &[ast::Stmt], sink: &mut Sink<'_>) {
    for statement in suite {
        let ast::Stmt::Assign(assign) = statement else {
            continue;
        };
        let ast::Expr::Tuple(tuple) = assign.value.as_ref() else {
            continue;
        };
        let [element] = tuple.elts.as_slice() else {
            continue;
        };
        if !matches!(element, ast::Expr::List(_) | ast::Expr::Dict(_)) {
            continue;
        }
        let tuple_start = u32::from(tuple.range().start()) as usize;
        let element_start = u32::from(element.range().start()) as usize;
        let after_equals = text
            .get(..tuple_start)
            .is_some_and(|head| head.trim_end().ends_with('='));
        let parenthesised = text
            .get(tuple_start..element_start)
            .is_some_and(|gap| gap.contains('('));
        if !after_equals || parenthesised {
            continue;
        }
        let name = assign
            .targets
            .iter()
            .find_map(|target| match target {
                ast::Expr::Name(name) => Some(name.id.as_str()),
                _ => None,
            })
            .unwrap_or("value");
        let kind = if matches!(element, ast::Expr::List(_)) {
            "list"
        } else {
            "dict"
        };
        sink.push(
            u32::from(assign.range().start()) as usize,
            "EB_PARAM_TUPLE_COMMA",
            format!("{name} is a one-element tuple holding a {kind}: drop the comma after its closing bracket (an edit probably left it behind)"),
        );
    }
}

// ---------------------------------------------------------------------------
// diff mode against the base (pre-edit) file

fn last_assigns(suite: &[ast::Stmt]) -> HashMap<&str, &ast::StmtAssign> {
    let mut out = HashMap::new();
    for statement in suite {
        if let ast::Stmt::Assign(assign) = statement {
            for target in &assign.targets {
                if let ast::Expr::Name(name) = target {
                    out.insert(name.id.as_str(), assign);
                }
            }
        }
    }
    out
}

fn str_value(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Constant(constant) => match &constant.value {
            ast::Constant::Str(value) => Some(value.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn expr_source<'t>(text: &'t str, expr: &ast::Expr) -> &'t str {
    let start = u32::from(expr.range().start()) as usize;
    let end = u32::from(expr.range().end()) as usize;
    text.get(start..end).unwrap_or("").trim()
}

/// A sanity_check_paths entry as the path text it stands for, so that
/// `'lib/x.%s' % SHLIB_EXT`, `'lib/x.' + SHLIB_EXT`, `f'lib/x.{SHLIB_EXT}'`
/// and `'lib/x.so'` all compare equal. `None` for entries that cannot be
/// compared without evaluating them (comprehensions, calls, ...).
fn render_entry(text: &str, expr: &ast::Expr) -> Option<String> {
    let rendered = match expr {
        ast::Expr::Constant(_) => str_value(expr)?.to_string(),
        ast::Expr::Name(name) => format!("{{{}}}", name.id.as_str()),
        ast::Expr::JoinedStr(joined) => {
            let mut out = String::new();
            for value in &joined.values {
                match value {
                    ast::Expr::Constant(_) => out.push_str(str_value(value)?),
                    ast::Expr::FormattedValue(field) => {
                        out.push('{');
                        out.push_str(expr_source(text, &field.value));
                        out.push('}');
                    }
                    _ => return None,
                }
            }
            out
        }
        ast::Expr::BinOp(binop) => match binop.op {
            ast::Operator::Add => {
                format!(
                    "{}{}",
                    render_entry(text, &binop.left)?,
                    render_entry(text, &binop.right)?
                )
            }
            ast::Operator::Mod => {
                let format = str_value(&binop.left)?;
                let rewrite = percent_rewrite(text, format, &binop.left, &binop.right)?;
                // f'...' -> ...
                rewrite.get(2..rewrite.len().saturating_sub(1))?.to_string()
            }
            _ => return None,
        },
        _ => return None,
    };
    let rendered = rendered.replace("{{", "{").replace("}}", "}");
    Some(match rendered.strip_suffix(".so") {
        Some(stem) => format!("{stem}.{{SHLIB_EXT}}"),
        None => rendered,
    })
}

fn list_entries<'e>(expr: &'e ast::Expr, out: &mut Vec<&'e ast::Expr>) -> bool {
    match expr {
        ast::Expr::List(list) => {
            out.extend(list.elts.iter());
            true
        }
        ast::Expr::Tuple(tuple) => {
            out.extend(tuple.elts.iter());
            true
        }
        ast::Expr::BinOp(binop) if matches!(binop.op, ast::Operator::Add) => {
            list_entries(&binop.left, out) && list_entries(&binop.right, out)
        }
        _ => false,
    }
}

/// One comparable `sanity_check_paths` entry.
struct SanityEntry {
    /// `files` or `dirs`.
    key: String,
    /// The path text the entry stands for (see [`render_entry`]).
    rendered: String,
    /// The entry as written.
    source: String,
    /// Byte offset of the entry in the text.
    offset: usize,
}

/// Each comparable sanity entry, in file order.
fn sanity_entries(text: &str, value: &ast::Expr) -> Option<Vec<SanityEntry>> {
    let ast::Expr::Dict(dict) = value else {
        return None;
    };
    let mut out = Vec::new();
    for (key, value) in dict.keys.iter().zip(&dict.values) {
        let Some(key) = key.as_ref().and_then(str_value) else {
            continue;
        };
        if key != "files" && key != "dirs" {
            continue;
        }
        let mut entries = Vec::new();
        if !list_entries(value, &mut entries) {
            continue;
        }
        for entry in entries {
            if let Some(rendered) = render_entry(text, entry) {
                out.push(SanityEntry {
                    key: key.to_string(),
                    rendered,
                    source: expr_source(text, entry).to_string(),
                    offset: u32::from(entry.range().start()) as usize,
                });
            }
        }
    }
    Some(out)
}

/// Values that `{name}` / `{version}` and the name and version templates
/// stand for at top level, taken from the recipe's own `name` and `version`.
fn name_version_values(assigns: &HashMap<&str, &ast::StmtAssign>) -> Vec<(String, String)> {
    let get = |key: &str| {
        assigns
            .get(key)
            .and_then(|assign| str_value(&assign.value))
            .map(str::to_string)
    };
    let mut out = Vec::new();
    if let Some(name) = get("name") {
        out.push(("{name}".to_string(), name.clone()));
        out.push(("%(name)s".to_string(), name.clone()));
        out.push(("%(namelower)s".to_string(), name.to_lowercase()));
    }
    if let Some(version) = get("version") {
        let parts: Vec<&str> = version.split('.').collect();
        out.push(("{version}".to_string(), version.clone()));
        out.push(("%(version)s".to_string(), version.clone()));
        out.push(("%(version_major)s".to_string(), parts[0].to_string()));
        if parts.len() > 1 {
            out.push(("%(version_minor)s".to_string(), parts[1].to_string()));
            out.push((
                "%(version_major_minor)s".to_string(),
                format!("{}.{}", parts[0], parts[1]),
            ));
        }
    }
    out
}

/// A rendered sanity entry with the name and version spellings replaced by
/// their values, so equivalent spellings compare equal.
fn canonical_entry(rendered: &str, values: &[(String, String)]) -> String {
    let mut out = rendered.to_string();
    for (spelling, value) in values {
        out = out.replace(spelling.as_str(), value);
    }
    out
}

/// A review line this close to a replacement sanity entry makes the change
/// of that entry's path one the review asked for.
const SANITY_REVIEW_NEAR: usize = 1;

/// Diff-mode errors against the base: a sanity_check_paths entry that is
/// gone (EB_SANITY_ENTRY_REMOVED), the whole parameter gone
/// (EB_SANITY_PATHS_REMOVED, a warning: dropping a default-equivalent
/// sanity_check_paths is a valid review request), and `easyblock` removed or
/// changed (EB_EASYBLOCK_CHANGED).
///
/// An entry whose path changed is not a removal when the review asked for
/// the change: a base entry with no equal entry now, paired with a new entry
/// under the same key that has no equal entry in the base, is accepted when
/// that new entry sits on (or next to) a review line, or in the lines that
/// replaced a review line. Reviewers fix paths in
/// suggestions (`'libfoo.so'` to `'lib/libfoo.so'`, an empty `''` dir to a
/// real one); reporting that as a lost entry made agents revert the
/// requested path (proof v7: every trial of one held-out case wrote the
/// suggested path, got EB_SANITY_ENTRY_REMOVED, and put the old path back).
fn diff_rules(
    text: &str,
    suite: &[ast::Stmt],
    base: &str,
    base_suite: &[ast::Stmt],
    review_lines: &[usize],
    sink: &mut Sink<'_>,
) {
    let now = last_assigns(suite);
    let before = last_assigns(base_suite);

    if let Some(old) = before
        .get("easyblock")
        .and_then(|assign| str_value(&assign.value))
    {
        match now.get("easyblock") {
            None => sink.push_at(
                1,
                1,
                "EB_EASYBLOCK_CHANGED",
                format!("easyblock = '{old}' was removed; the easyconfig now builds with a different easyblock. Keep it unless the review explicitly asks to change the easyblock"),
            ),
            Some(assign) => {
                let new = str_value(&assign.value);
                if new != Some(old) {
                    sink.push(
                        u32::from(assign.range.start()) as usize,
                        "EB_EASYBLOCK_CHANGED",
                        format!("easyblock changed from '{old}' to {}; keep it unless the review explicitly asks to change the easyblock", expr_source(text, &assign.value)),
                    );
                }
            }
        }
    }

    let Some(old_sanity) = before.get("sanity_check_paths") else {
        return;
    };
    let Some(new_sanity) = now.get("sanity_check_paths") else {
        sink.push_at(
            1,
            1,
            "EB_SANITY_PATHS_REMOVED",
            "sanity_check_paths was removed. That is right only when the review asks for it (for example because it equals the easyblock's default); otherwise restore it".into(),
        );
        return;
    };
    let (Some(old_entries), Some(new_entries)) = (
        sanity_entries(base, &old_sanity.value),
        sanity_entries(text, &new_sanity.value),
    ) else {
        return;
    };
    // Compare the paths the entries stand for, not their spelling: at top
    // level `f'include/{name}/{name}.h'`, `'include/%(name)s/%(name)s.h'` and
    // `'include/spdlog/spdlog.h'` are the same file (proof v5, PR 24391: the
    // reviewer's template suggestion was reported as three lost entries).
    let old_values = name_version_values(&before);
    let new_values = name_version_values(&now);
    // (key, canonical path, offset) of new entries not yet matched.
    let mut remaining: Vec<(String, String, usize)> = new_entries
        .into_iter()
        .map(|entry| {
            (
                entry.key,
                canonical_entry(&entry.rendered, &new_values),
                entry.offset,
            )
        })
        .collect();
    let mut lost: Vec<(String, String)> = Vec::new();
    for entry in old_entries {
        let rendered = canonical_entry(&entry.rendered, &old_values);
        if let Some(at) = remaining.iter().position(|(new_key, new_rendered, _)| {
            *new_key == entry.key && *new_rendered == rendered
        }) {
            remaining.remove(at);
            continue;
        }
        lost.push((entry.key, entry.source));
    }
    let index = sink.index;
    // Review lines may be in the base's numbering (as the review showed
    // them) or in the current one: a review line inside a changed region
    // covers every line that replaced it (a one-line list re-wrapped over
    // five lines keeps its anchor).
    let changed = super::diffscope::hunks(base, text);
    let on_review_line = |offset: usize| {
        let line = index.line(offset);
        review_lines.iter().any(|review| {
            review.abs_diff(line) <= SANITY_REVIEW_NEAR
                || changed.iter().any(|hunk| {
                    let new_hi = hunk.new_start + hunk.added.len().saturating_sub(1);
                    let old_lo = hunk
                        .old_start
                        .saturating_sub(usize::from(hunk.removed.is_empty()));
                    let old_hi = hunk.old_start + hunk.removed.len().saturating_sub(1);
                    (hunk.new_start..=new_hi).contains(&line)
                        && ((old_lo..=old_hi).contains(review)
                            || (hunk.new_start..=new_hi).contains(review))
                })
        })
    };
    for (key, source) in lost {
        // The path was changed, not dropped, and the review is about the
        // line that holds the new path.
        if let Some(at) = remaining
            .iter()
            .position(|(new_key, _, offset)| *new_key == key && on_review_line(*offset))
        {
            remaining.remove(at);
            continue;
        }
        sink.push(
            u32::from(new_sanity.range.start()) as usize,
            "EB_SANITY_ENTRY_REMOVED",
            format!("sanity_check_paths '{key}' lost {source} from the base; converting an entry's formatting must keep the same path. Restore it unless the review asks to drop it. If the review asked to change this path, keep the new path and pass the line that holds it as review_lines; a path change the review asked for is not a removal"),
        );
    }
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
                let fragment = super::review::source_fragment(self.text, binop.range);
                let advice = match percent_rewrite(self.text, value, &binop.left, &binop.right) {
                    Some(rewrite) => format!("`{fragment}` becomes `{rewrite}`"),
                    None => format!(
                        "`{fragment}`: {}",
                        super::review::percent_guidance(&binop.right)
                    ),
                };
                self.found.push((
                    offset,
                    "EB_FMT_PERCENT",
                    format!("%-formatting of a string literal; use an f-string instead (e.g. f'lib/libfoo.{{SHLIB_EXT}}') and keep %(...)s templates as they are{hint}. {advice}"),
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

/// A version string that stands in for "I do not know", not a release.
/// `''` is what the rule was written for; `'0'`, `'x'` and `'TODO'` are the
/// same failure (proof v5, PR 24538: a trial wrote `('Cython', '0')`).
fn is_placeholder_version(version: &str) -> bool {
    let version = version.trim();
    version.is_empty()
        || version == "0"
        || version.eq_ignore_ascii_case("x")
        || version.eq_ignore_ascii_case("todo")
}

/// EB_DEP_EMPTY_VERSION: a (build)dependency tuple whose version is `''`,
/// `'0'`, `'x'` or `'TODO'`. EasyBuild cannot resolve it; the version has to
/// be one that exists for the toolchain generation (look it up in the robot
/// tree).
fn empty_dependency_versions(assigns: &HashMap<&str, Vec<&ast::StmtAssign>>, sink: &mut Sink<'_>) {
    for key in ["dependencies", "builddependencies"] {
        for statement in assigns.get(key).into_iter().flatten() {
            let ast::Expr::List(list) = statement.value.as_ref() else {
                continue;
            };
            for element in &list.elts {
                let ast::Expr::Tuple(tuple) = element else {
                    continue;
                };
                let (Some(name), Some(version)) = (tuple.elts.first(), tuple.elts.get(1)) else {
                    continue;
                };
                let ast::Expr::Constant(name) = name else {
                    continue;
                };
                let ast::Constant::Str(name) = &name.value else {
                    continue;
                };
                let ast::Expr::Constant(version) = version else {
                    continue;
                };
                let ast::Constant::Str(version) = &version.value else {
                    continue;
                };
                if is_placeholder_version(version) {
                    sink.push(
                        u32::from(element.range().start()) as usize,
                        "EB_DEP_EMPTY_VERSION",
                        format!("{key} entry for {name} has version '{version}', which is not a real version; use a version that exists for this toolchain generation (look it up with eb_dep_versions) instead of '', '0', 'x' or 'TODO'"),
                    );
                }
            }
        }
    }
}

/// The f-string that replaces `'...%s...' % args`, built from the flagged
/// expression itself: each `%s` takes the source text of its argument and
/// `%%` becomes `%` (so `%%(version)s` turns back into the template). `None`
/// when the rewrite is not mechanical (other conversions, a dict or a name
/// that may hold a tuple of several values, escapes, or quotes that would
/// clash).
pub(super) fn percent_rewrite(
    text: &str,
    format: &str,
    left: &ast::Expr,
    right: &ast::Expr,
) -> Option<String> {
    let source = |expr: &ast::Expr| -> Option<String> {
        let start = u32::from(expr.range().start()) as usize;
        let end = u32::from(expr.range().end()) as usize;
        let raw = text.get(start..end)?;
        (!raw.contains('\n')).then(|| raw.trim().to_string())
    };
    let args: Vec<String> = match right {
        ast::Expr::Tuple(tuple) => tuple.elts.iter().map(source).collect::<Option<_>>()?,
        ast::Expr::Dict(_) | ast::Expr::List(_) | ast::Expr::Constant(_) => return None,
        other => vec![source(other)?],
    };
    if format.chars().any(|c| c == '\\' || c.is_control()) {
        return None;
    }
    // Keep the quote the literal was written with when it still fits.
    let written = text
        .get(u32::from(left.range().start()) as usize..)
        .and_then(|rest| rest.chars().find(|c| *c == '\'' || *c == '"'))
        .unwrap_or('\'');
    let other = if written == '"' { '\'' } else { '"' };
    let quote = if !format.contains(written) {
        written
    } else if !format.contains(other) {
        other
    } else {
        return None;
    };
    if args.iter().any(|arg| arg.contains(quote)) {
        return None;
    }
    let mut out = String::from("f");
    out.push(quote);
    let mut next = args.iter();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => match chars.next() {
                Some('%') => out.push('%'),
                Some('s') => {
                    out.push('{');
                    out.push_str(next.next()?);
                    out.push('}');
                }
                _ => return None,
            },
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            other => out.push(other),
        }
    }
    if next.next().is_some() {
        return None;
    }
    out.push(quote);
    Some(out)
}

/// Number of entries an easyconfig list value holds. Each list or tuple
/// element counts once, whether it is a string, a dict (`{'name': ...}` or
/// `{'file': 'sha256'}`), a tuple of alternatives, or `None`; `a + b` adds up.
fn entry_count(value: &ast::Expr) -> usize {
    match value {
        ast::Expr::List(list) => list.elts.len(),
        ast::Expr::Tuple(tuple) => tuple.elts.len(),
        ast::Expr::BinOp(binop) if matches!(binop.op, ast::Operator::Add) => {
            entry_count(&binop.left) + entry_count(&binop.right)
        }
        ast::Expr::Constant(constant) if matches!(constant.value, ast::Constant::None) => 0,
        _ => 1,
    }
}

/// EB_CHECKSUM_COUNT: port of `EasyBlock.check_checksums_for` (top level only).
fn checksum_count(
    assigns: &HashMap<&str, Vec<&ast::StmtAssign>>,
    file_name: &str,
    sink: &mut Sink<'_>,
) {
    let count = |key: &str| -> usize {
        assigns
            .get(key)
            .and_then(|statements| statements.last())
            .map_or(0, |statement| entry_count(&statement.value))
    };
    // The Cargo easyblocks (Cargo, CargoPythonPackage, CargoPythonBundle)
    // turn every `crates` entry into a source with its own checksum, so the
    // checksums list covers sources + crates + patches.
    let sources = count("sources") + count("data_sources") + count("crates");
    // Easyblock options that add a patch file with its own checksum, such as
    // the Python easyblock's patch_ctypes_ld_library_path, count as patches.
    let extra_patches: usize = assigns
        .keys()
        .filter(|key| key.starts_with("patch_"))
        .map(|key| count(key))
        .sum();
    let patches = count("patches") + extra_patches;
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

    fn has(findings: &[StyleFinding], code: &str) -> bool {
        findings.iter().any(|finding| finding.code == code)
    }

    #[test]
    fn extension_filter_and_easyblock_templates_are_known() {
        let text = "exts_filter = (\"python -s -c 'import %(ext_name)s'\", '')\n\
                    exts_default_options = {'source_tmpl': '%(ext_name)s-%(ext_version)s.tar.gz'}\n\
                    sources = ['%(namelower)s-linux-%(cudnnarch)s-%(version)s.tar.xz']\n\
                    runtest = '%(python)s run_test.py %(excluded_tests)s'\n";
        let findings = lint(text);
        assert!(!has(&findings, "EB_TEMPLATE_UNKNOWN"), "{findings:?}");
        // A real typo is still caught, with a suggestion.
        let typo = lint("configopts = '-DX=%(cuda__cc_cmake)s'\n");
        assert!(
            typo.iter()
                .any(|finding| finding.code == "EB_TEMPLATE_UNKNOWN"
                    && finding.message.contains("%(cuda_cc_cmake)s")),
            "{typo:?}"
        );
    }

    #[test]
    fn python_percent_dict_keys_are_not_templates() {
        let text = "local_sed = r'sed -i \"s/%(l)s_VERSION/x/\" %(f)s && '\n\
                    preconfigopts = local_sed % {'l': 'LIBXML2', 'f': 'CMakeLists.txt'}\n\
                    buildopts = '%(cc)s -O2' % {'cc': 'gcc'}\n";
        let findings = lint(text);
        assert!(!has(&findings, "EB_TEMPLATE_UNKNOWN"), "{findings:?}");
    }

    #[test]
    fn sanity_commands_must_not_use_the_build_or_install_dir() {
        let builddir = "sanity_check_commands = [\n    \"mkdir -p %(builddir)s && cp -a %(installdir)s/ex %(builddir)s && \"\n    \"cd %(builddir)s/ex && prog\",\n]\n";
        let found = lint(builddir);
        assert!(has(&found, "EB_SANITY_CMD_BUILDDIR"), "{found:?}");
        let install = "sanity_check_commands = [\"cd %(installdir)s/ex && prog\"]\n";
        assert!(has(&lint(install), "EB_SANITY_CMD_CD_INSTALLDIR"));
        let tmp = "sanity_check_commands = [\"cd $(mktemp -d) && cp -r %(installdir)s/ex . && cd ex && prog\"]\n";
        assert!(
            !has(&lint(tmp), "EB_SANITY_CMD_CD_INSTALLDIR"),
            "{:?}",
            lint(tmp)
        );
        assert!(!has(&lint(tmp), "EB_SANITY_CMD_BUILDDIR"));
        // Elsewhere %(builddir)s is fine.
        let other = "preconfigopts = 'cd %(builddir)s && '\n";
        assert!(!has(&lint(other), "EB_SANITY_CMD_BUILDDIR"));
    }

    #[test]
    fn checksum_count_includes_easyblock_patch_options_and_dict_entries() {
        let python = "sources = [SOURCE_TGZ]\n\
                      patch_ctypes_ld_library_path = 'Python-3.11.5-custom-ctypes.patch'\n\
                      checksums = [\n    {'Python-3.10.8.tgz': 'aa'},\n    {'Python-3.11.5-custom-ctypes.patch': 'bb'},\n]\n";
        assert!(
            !has(&lint(python), "EB_CHECKSUM_COUNT"),
            "{:?}",
            lint(python)
        );
        let mixed = "sources = [SOURCE_TAR_GZ] + [{'filename': 'extra.tar.gz', 'extract_cmd': 'tar xf %s'}]\n\
                     patches = [('fix.patch', 1), {'name': 'other.patch', 'alt_location': 'x'}]\n\
                     checksums = ['a', 'b', 'c', ('d', 'e')]\n";
        assert!(!has(&lint(mixed), "EB_CHECKSUM_COUNT"), "{:?}", lint(mixed));
        let short = "sources = [SOURCE_TGZ]\npatches = ['fix.patch']\nchecksums = ['a']\n";
        assert!(has(&lint(short), "EB_CHECKSUM_COUNT"));
        // Cargo easyblocks add one source per crate.
        let cargo = "easyblock = 'CargoPythonPackage'\nsources = ['v1.0.tar.gz']\n\
                     crates = [('a', '1.0'), ('b', '2.0')]\n\
                     checksums = [{'v1.0.tar.gz': 'x'}, {'a-1.0.tar.gz': 'y'}, {'b-2.0.tar.gz': 'z'}]\n";
        assert!(!has(&lint(cargo), "EB_CHECKSUM_COUNT"), "{:?}", lint(cargo));
        let cargo_short = "sources = ['v1.0.tar.gz']\ncrates = [('a', '1.0')]\nchecksums = ['x']\n";
        assert!(has(&lint(cargo_short), "EB_CHECKSUM_COUNT"));
    }

    #[test]
    fn hardcoded_so_only_for_library_paths_and_sanity_checks() {
        let build_tree = "files_to_copy = [(['bin/7z', 'bin/7z.so', 'bin/Codecs'], 'libexec')]\n";
        assert!(
            !has(&lint(build_tree), "EB_SHLIB_HARDCODED"),
            "{:?}",
            lint(build_tree)
        );
        let lib_path = "configopts = '-DX_LIBRARIES=$EBROOTX/lib64/libx.so'\n";
        assert!(has(&lint(lib_path), "EB_SHLIB_HARDCODED"));
        let in_lib = "postinstallcmds = ['ln -s libfoo.so.1 %(installdir)s/lib/libfoo.so']\n";
        assert!(has(&lint(in_lib), "EB_SHLIB_HARDCODED"));
        let sanity = "sanity_check_paths = {\n    'files': ['bin/7z.so'],\n    'dirs': [],\n}\n";
        assert!(
            has(&lint(sanity), "EB_SHLIB_HARDCODED"),
            "{:?}",
            lint(sanity)
        );
        let versioned = "files_to_copy = ['lib/libfoo.so.1']\n";
        assert!(!has(&lint(versioned), "EB_SHLIB_HARDCODED"));
    }

    #[test]
    fn percent_suggestion_is_built_from_the_flagged_expression() {
        let text = "local_makefile = 'makefile.linux_amd64'\n\
                    prebuildopts = \"cp %s makefile.linux &&\" % local_makefile\n\
                    files = ['libexec/7z.%s' % SHLIB_EXT]\n\
                    sources = ['BEAST.v%%(version)s.Linux.%s.tgz' % local_archs[ARCH]]\n\
                    versionsuffix = '-%s%s' % (local_a, local_b)\n";
        let findings = lint(text);
        let message = |line: usize| {
            findings
                .iter()
                .find(|finding| finding.line == line && finding.code == "EB_FMT_PERCENT")
                .unwrap_or_else(|| panic!("no EB_FMT_PERCENT on {line}: {findings:?}"))
                .message
                .clone()
        };
        let prebuild = message(2);
        assert!(
            prebuild.contains("becomes `f\"cp {local_makefile} makefile.linux &&\"`"),
            "{prebuild}"
        );
        assert!(!prebuild.contains("7z"), "{prebuild}");
        assert!(message(3).contains("becomes `f'libexec/7z.{SHLIB_EXT}'`"));
        assert!(
            message(4).contains("becomes `f'BEAST.v%(version)s.Linux.{local_archs[ARCH]}.tgz'`")
        );
        assert!(message(5).contains("becomes `f'-{local_a}{local_b}'`"));
    }

    #[test]
    fn empty_dependency_version_is_flagged() {
        let text = "builddependencies = [\n    ('binutils', '2.42'),\n    ('Cython', ''),\n]\n";
        let findings = lint(text);
        let hits: Vec<_> = findings
            .iter()
            .filter(|finding| finding.code == "EB_DEP_EMPTY_VERSION")
            .collect();
        assert_eq!(hits.len(), 1, "{findings:?}");
        assert_eq!(hits[0].line, 3);
        assert_eq!(hits[0].severity, super::super::Severity::Error);
        assert!(!super::super::findings_error_free(&findings));
        assert!(hits[0].message.contains("Cython"));
    }

    #[test]
    fn placeholder_dependency_versions_are_errors() {
        // 24538 wrote ('Cython', '0') after eb_dep_versions did not list the
        // GCCcore version. '', '0', 'x' and 'TODO' are the same kind of guess.
        for version in ["", "0", " 0 ", "x", "X", "TODO", "todo"] {
            let text = format!("dependencies = [('Cython', '{version}')]\n");
            let findings = lint(&text);
            let hits: Vec<_> = findings
                .iter()
                .filter(|finding| finding.code == "EB_DEP_EMPTY_VERSION")
                .collect();
            assert_eq!(hits.len(), 1, "{version:?} {findings:?}");
            assert_eq!(hits[0].severity, super::super::Severity::Error);
            assert!(!super::super::findings_clean(&findings), "{version:?}");
        }
        for version in ["0.0", "10", "3.0.10"] {
            let text = format!("builddependencies = [('Cython', '{version}')]\n");
            let findings = lint(&text);
            assert!(
                !has(&findings, "EB_DEP_EMPTY_VERSION"),
                "{version:?} {findings:?}"
            );
        }
    }

    #[test]
    fn formatting_findings_keep_a_file_from_being_clean() {
        let leftover = lint("files = ['libexec/7z.%s' % SHLIB_EXT]\n");
        assert!(super::super::findings_error_free(&leftover));
        assert!(!super::super::findings_clean(&leftover), "{leftover:?}");
        let other_warning = lint("files_to_copy = ['lib/libfoo.so']\n");
        assert!(has(&other_warning, "EB_SHLIB_HARDCODED"));
        assert!(
            super::super::findings_clean(&other_warning),
            "{other_warning:?}"
        );
        let fixed = lint("files = [f'libexec/7z.{SHLIB_EXT}']\n");
        assert!(super::super::findings_clean(&fixed), "{fixed:?}");
    }

    fn lint_against(text: &str, base: &str) -> Vec<StyleFinding> {
        let mut out = Vec::new();
        extend_rules(
            text,
            &LintOptions {
                base: Some(base),
                ..LintOptions::default()
            },
            &mut out,
        );
        out
    }

    const TCL_BASE: &str = "easyblock = 'ConfigureMake'\n\
        sanity_check_paths = {\n    \
        'files': ['bin/tclsh', 'lib/libtcl%%(version_major)s.%%(version_minor)s.%s' % SHLIB_EXT, 'lib/libtcl.so'],\n    \
        'dirs': ['include'],\n}\n";

    #[test]
    fn escaped_template_inside_an_fstring_is_an_error() {
        let findings = lint("files = [f'lib/libtcl%%(version_major)s.{SHLIB_EXT}']\n");
        let hit = findings
            .iter()
            .find(|finding| finding.code == "EB_FSTRING_ESCAPED_TEMPLATE")
            .unwrap_or_else(|| panic!("{findings:?}"));
        assert_eq!(hit.severity, super::super::Severity::Error);
        // Plain %%(...)s with the % operator and %(...)s in an f-string are fine.
        assert!(!has(
            &lint("files = ['lib/libtcl%%(version_major)s.%s' % SHLIB_EXT]\n"),
            "EB_FSTRING_ESCAPED_TEMPLATE"
        ));
        assert!(!has(
            &lint("files = [f'lib/libtcl%(version_major)s.{SHLIB_EXT}']\n"),
            "EB_FSTRING_ESCAPED_TEMPLATE"
        ));
    }

    #[test]
    fn converting_sanity_entries_keeps_them_but_dropping_one_is_an_error() {
        let converted = "easyblock = 'ConfigureMake'\n\
            sanity_check_paths = {\n    \
            'files': ['bin/tclsh', f'lib/libtcl%(version_major)s.%(version_minor)s.{SHLIB_EXT}', f'lib/libtcl.{SHLIB_EXT}'],\n    \
            'dirs': ['include'],\n}\n";
        let findings = lint_against(converted, TCL_BASE);
        assert!(!has(&findings, "EB_SANITY_ENTRY_REMOVED"), "{findings:?}");

        // The collapse seen in proof v4: the versioned library entry is gone.
        let collapsed = "easyblock = 'ConfigureMake'\n\
            sanity_check_paths = {\n    \
            'files': ['bin/tclsh', f'lib/libtcl.{SHLIB_EXT}'],\n    \
            'dirs': ['include'],\n}\n";
        let findings = lint_against(collapsed, TCL_BASE);
        let lost: Vec<_> = findings
            .iter()
            .filter(|finding| finding.code == "EB_SANITY_ENTRY_REMOVED")
            .collect();
        assert_eq!(lost.len(), 1, "{findings:?}");
        assert_eq!(lost[0].severity, super::super::Severity::Error);
        assert!(
            lost[0].message.contains("libtcl%%(version_major)s"),
            "{}",
            lost[0].message
        );

        // Keeping %% inside the f-string is a different path and an error too.
        let escaped = converted.replace(
            "libtcl%(version_major)s.%(version_minor)s",
            "libtcl%%(version_major)s.%%(version_minor)s",
        );
        let findings = lint_against(&escaped, TCL_BASE);
        assert!(
            has(&findings, "EB_FSTRING_ESCAPED_TEMPLATE"),
            "{findings:?}"
        );
        assert!(has(&findings, "EB_SANITY_ENTRY_REMOVED"), "{findings:?}");
    }

    #[test]
    fn respelling_name_and_version_is_not_a_lost_sanity_entry() {
        let base = "name = 'spdlog'\nversion = '1.15.3'\n\
            sanity_check_paths = {\n    \
            'files': [f'include/{name}/{name}.h', 'lib/libspdlog.a', 'lib/libspdlog.%s' % SHLIB_EXT],\n    \
            'dirs': [],\n}\n";
        let suggested = "name = 'spdlog'\nversion = '1.15.3'\n\
            sanity_check_paths = {\n    \
            'files': ['include/%(name)s/%(name)s.h', 'lib/lib%(name)s.a', f'lib/lib%(name)s.{SHLIB_EXT}'],\n    \
            'dirs': [],\n}\n";
        let findings = lint_against(suggested, base);
        assert!(!has(&findings, "EB_SANITY_ENTRY_REMOVED"), "{findings:?}");
        // A really different path still counts.
        let dropped = suggested.replace("'lib/lib%(name)s.a', ", "");
        assert!(has(
            &lint_against(&dropped, base),
            "EB_SANITY_ENTRY_REMOVED"
        ));
    }

    fn lint_against_review(text: &str, base: &str, review_lines: &[usize]) -> Vec<StyleFinding> {
        let mut out = Vec::new();
        extend_rules(
            text,
            &LintOptions {
                base: Some(base),
                review_lines,
                ..LintOptions::default()
            },
            &mut out,
        );
        out
    }

    #[test]
    fn a_path_change_on_a_review_line_is_not_a_lost_sanity_entry() {
        let base = "easyblock = 'CMakeMake'\n\
            sanity_check_paths = {\n    \
            'files': ['include/foo.h', f'libfoo-core.{SHLIB_EXT}', f'lib/libfoo-extra.{SHLIB_EXT}'],\n    \
            'dirs': [''],\n}\n";
        // The suggestion wraps the list and adds the missing lib/ prefix.
        let fixed = "easyblock = 'CMakeMake'\n\
            sanity_check_paths = {\n    \
            'files': [\n        \
            'include/foo.h',\n        \
            f'lib/libfoo-core.{SHLIB_EXT}',\n        \
            f'lib/libfoo-extra.{SHLIB_EXT}',\n    \
            ],\n    \
            'dirs': [''],\n}\n";
        // Review lines remapped onto the replacement lines (3-8).
        let findings = lint_against_review(fixed, base, &[3, 4, 5, 6, 7, 8]);
        assert!(!has(&findings, "EB_SANITY_ENTRY_REMOVED"), "{findings:?}");
        // Without review lines the change is still reported, with the way out.
        let findings = lint_against(fixed, base);
        let hit = findings
            .iter()
            .find(|finding| finding.code == "EB_SANITY_ENTRY_REMOVED")
            .unwrap_or_else(|| panic!("{findings:?}"));
        assert!(hit.message.contains("not a removal"), "{}", hit.message);
        // The review line as the review showed it (the one-line list, now
        // re-wrapped over lines 3-7) covers the lines that replaced it.
        assert!(!has(
            &lint_against_review(fixed, base, &[3]),
            "EB_SANITY_ENTRY_REMOVED"
        ));
        // A review line far from the changed entry does not excuse it.
        assert!(has(
            &lint_against_review(fixed, base, &[1]),
            "EB_SANITY_ENTRY_REMOVED"
        ));
        // An empty dir replaced by the requested one, on the review line.
        let dirs = base.replace("'dirs': ['']", "'dirs': ['share/man/man1']");
        assert!(!has(
            &lint_against_review(&dirs, base, &[4]),
            "EB_SANITY_ENTRY_REMOVED"
        ));
        // Dropping an entry outright is still an error, review line or not:
        // nothing new under the same key stands in for it.
        let dropped = base.replace(", f'libfoo-core.{SHLIB_EXT}'", "");
        assert!(has(
            &lint_against_review(&dropped, base, &[3]),
            "EB_SANITY_ENTRY_REMOVED"
        ));
    }

    #[test]
    fn removing_sanity_check_paths_is_a_warning() {
        let findings = lint_against("easyblock = 'ConfigureMake'\n", TCL_BASE);
        let hit = findings
            .iter()
            .find(|finding| finding.code == "EB_SANITY_PATHS_REMOVED")
            .unwrap_or_else(|| panic!("{findings:?}"));
        assert_eq!(hit.severity, super::super::Severity::Warning);
        assert!(super::super::findings_clean(&findings), "{findings:?}");
    }

    #[test]
    fn removing_or_changing_the_easyblock_is_an_error() {
        let base = "easyblock = 'PythonPackage'\nname = 'x'\n";
        let removed = lint_against("name = 'x'\n", base);
        assert!(has(&removed, "EB_EASYBLOCK_CHANGED"), "{removed:?}");
        let changed = lint_against("easyblock = 'PythonBundle'\nname = 'x'\n", base);
        let hit = changed
            .iter()
            .find(|finding| finding.code == "EB_EASYBLOCK_CHANGED")
            .unwrap_or_else(|| panic!("{changed:?}"));
        assert_eq!(hit.line, 1);
        assert!(hit.message.contains("PythonPackage"));
        assert!(!has(&lint_against(base, base), "EB_EASYBLOCK_CHANGED"));
        // Without a base there is nothing to compare.
        assert!(!has(&lint("name = 'x'\n"), "EB_EASYBLOCK_CHANGED"));
    }

    #[test]
    fn f541_counts_implicit_concatenation_as_one_string() {
        assert!(lint("x = (f'a{SHLIB_EXT}' 'b')\n").is_empty());
        assert_eq!(codes(&lint("x = (f'a' 'b')\n")), [(1, "F541".to_string())]);
    }

    #[test]
    fn a_trailing_comma_after_a_top_level_list_or_dict_is_an_error() {
        let text = "name = 'x'\nsanity_check_paths = {\n    'files': ['bin/x'],\n    'dirs': [],\n},\nbuilddependencies = [('CMake', '3.29.3')],\n";
        let found = codes(&lint(text));
        assert!(
            found.contains(&(2, "EB_PARAM_TUPLE_COMMA".to_string())),
            "{found:?}"
        );
        assert!(
            found.contains(&(6, "EB_PARAM_TUPLE_COMMA".to_string())),
            "{found:?}"
        );
        let fine = "name = 'x'\nsanity_check_paths = {\n    'files': ['bin/x'],\n    'dirs': [],\n}\npair = ([1], [2])\nkept = ([1],)\nversions = '1', '2'\n";
        assert!(
            !has(&lint(fine), "EB_PARAM_TUPLE_COMMA"),
            "{:?}",
            codes(&lint(fine))
        );
    }
}
