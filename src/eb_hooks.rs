//! `PARALLELISM_LIMITS` from an EESSI `eb_hooks.py`, read as Python source.
//!
//! software-layer-scripts keeps its per-package parallelism limits in a
//! module-level dict that `post_ready_hook` reads. A key whose value is an
//! `(op, argument)` tuple is a CPU target's own limit. A key whose value is
//! a dict is a software name, and that dict's keys are CPU targets or
//! `'*'`. A key is a string or a module-level string constant such as
//! `CPU_TARGET_A64FX`. The ops are `divide_by_factor` and `set_maximum`.
//! The file is parsed, never run. A row this reader cannot model is an
//! error that names its line, because the parallel levels would be wrong
//! without it.

use crate::oom_profile::{DivideFactor, HookLimit, LimitOp};
use anyhow::{anyhow, bail, Result};
use rustpython_parser::ast::{self, Ranged};
use rustpython_parser::text_size::TextSize;
use rustpython_parser::Parse;
use std::collections::BTreeMap;

/// The dict `post_ready_hook` looks a build up in.
pub const LIMITS_NAME: &str = "PARALLELISM_LIMITS";

/// `max(1, parallel // factor)` in `eb_hooks.py`.
const DIVIDE_OP: &str = "divide_by_factor";

/// `min(parallel, max_value)` in `eb_hooks.py`.
const MAXIMUM_OP: &str = "set_maximum";

/// What `stack oom` reads from one `eb_hooks.py`.
#[derive(Debug, Clone, PartialEq)]
pub struct HooksFile {
    /// `PARALLELISM_LIMITS` rows. A key the dict repeats keeps its last
    /// value, as Python does.
    pub limits: Vec<HookLimit>,
}

/// Read `PARALLELISM_LIMITS` from `text`.
///
/// `file` names the file in errors and in each row's location. The last
/// module-level assignment to `PARALLELISM_LIMITS` is the one
/// `post_ready_hook` sees, and its keys resolve through the string
/// constants bound before it.
pub fn parse_hooks(text: &str, file: &str) -> Result<HooksFile> {
    let suite = ast::Suite::parse(text, file).map_err(|error| {
        let offset = offset(error.offset).min(text.len());
        anyhow!(
            "{file}:{}: not valid Python: {}",
            line_at(text, offset),
            error.error
        )
    })?;
    let (value, constants) =
        find_limits(&suite).ok_or_else(|| anyhow!("{file}: no module-level {LIMITS_NAME}"))?;
    let reader = Reader {
        text,
        file,
        constants: &constants,
    };
    let ast::Expr::Dict(dict) = value else {
        bail!(
            "{file}:{}: {LIMITS_NAME} is not a dict literal",
            reader.line(value)
        );
    };
    let limits = reader.limits(dict)?;
    Ok(HooksFile { limits })
}

/// The last module-level `PARALLELISM_LIMITS = ...`, and the string
/// constants bound before it.
fn find_limits(suite: &[ast::Stmt]) -> Option<(&ast::Expr, BTreeMap<String, String>)> {
    let mut constants = BTreeMap::new();
    let mut found = None;
    for statement in suite {
        let (targets, value): (Vec<&ast::Expr>, &ast::Expr) = match statement {
            ast::Stmt::Assign(assign) => (assign.targets.iter().collect(), &assign.value),
            ast::Stmt::AnnAssign(assign) => match &assign.value {
                Some(value) => (vec![&assign.target], value),
                None => continue,
            },
            _ => continue,
        };
        for target in targets {
            let ast::Expr::Name(name) = target else {
                continue;
            };
            if name.id.as_str() == LIMITS_NAME {
                found = Some((value, constants.clone()));
            } else if let Some(text) = string_literal(value) {
                constants.insert(name.id.to_string(), text.to_string());
            } else {
                constants.remove(name.id.as_str());
            }
        }
    }
    found
}

fn string_literal(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Constant(ast::ExprConstant {
            value: ast::Constant::Str(text),
            ..
        }) => Some(text),
        _ => None,
    }
}

enum Number {
    Int(u32),
    Float(f64),
}

fn number(expr: &ast::Expr) -> Option<Number> {
    let ast::Expr::Constant(constant) = expr else {
        return None;
    };
    match &constant.value {
        ast::Constant::Int(value) => value.to_string().parse().ok().map(Number::Int),
        ast::Constant::Float(value) => Some(Number::Float(*value)),
        _ => None,
    }
}

struct Reader<'a> {
    text: &'a str,
    file: &'a str,
    constants: &'a BTreeMap<String, String>,
}

impl Reader<'_> {
    fn line(&self, node: &impl Ranged) -> usize {
        line_at(self.text, offset(node.start()))
    }

    fn source(&self, node: &impl Ranged) -> &str {
        let range = node.range();
        self.text
            .get(offset(range.start())..offset(range.end()))
            .unwrap_or("")
    }

    fn key(&self, key: &ast::Expr) -> Result<String> {
        if let Some(text) = string_literal(key) {
            return Ok(text.to_string());
        }
        let at = format!("{}:{}", self.file, self.line(key));
        match key {
            ast::Expr::Name(name) => {
                self.constants
                    .get(name.id.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "{at}: {LIMITS_NAME} key {} is not a module-level string constant",
                            name.id
                        )
                    })
            }
            other => bail!(
                "{at}: {LIMITS_NAME} key {} is not a string",
                self.source(other)
            ),
        }
    }

    fn limits(&self, dict: &ast::ExprDict) -> Result<Vec<HookLimit>> {
        let mut rows: Vec<HookLimit> = Vec::new();
        for (key, value) in dict.keys.iter().zip(&dict.values) {
            let Some(key) = key else {
                bail!(
                    "{}:{}: {LIMITS_NAME} unpacks a dict with **; only written-out rows are read",
                    self.file,
                    self.line(value)
                );
            };
            let name = self.key(key)?;
            rows.retain(|row| {
                row.software.as_deref() != Some(name.as_str())
                    && !(row.software.is_none() && row.target == name)
            });
            match value {
                ast::Expr::Tuple(tuple) => {
                    if name == "*" {
                        bail!(
                            "{}:{}: a {LIMITS_NAME} limit for every build needs a CPU target, not '*'",
                            self.file,
                            self.line(key)
                        );
                    }
                    let op = self.op(tuple, "every build", &name)?;
                    rows.push(HookLimit {
                        software: None,
                        target: name,
                        op,
                        line: Some(self.line(tuple)),
                    });
                }
                ast::Expr::Dict(inner) => {
                    if inner.keys.is_empty() {
                        bail!(
                            "{}:{}: {LIMITS_NAME} entry for {name} is empty, and post_ready_hook then skips even the CPU target's own limit",
                            self.file,
                            self.line(value)
                        );
                    }
                    for (target, row) in inner.keys.iter().zip(&inner.values) {
                        let Some(target) = target else {
                            bail!(
                                "{}:{}: {LIMITS_NAME} entry for {name} unpacks a dict with **; only written-out rows are read",
                                self.file,
                                self.line(row)
                            );
                        };
                        let target = self.key(target)?;
                        let ast::Expr::Tuple(tuple) = row else {
                            bail!(
                                "{}:{}: {LIMITS_NAME} row for {name} on {target} is {}, not an (op, argument) tuple",
                                self.file,
                                self.line(row),
                                self.source(row)
                            );
                        };
                        let op = self.op(tuple, &name, &target)?;
                        rows.retain(|limit| {
                            !(limit.software.as_deref() == Some(name.as_str())
                                && limit.target == target)
                        });
                        rows.push(HookLimit {
                            software: Some(name.clone()),
                            target,
                            op,
                            line: Some(self.line(tuple)),
                        });
                    }
                }
                other => bail!(
                    "{}:{}: {LIMITS_NAME} value for {name} is {}, neither an (op, argument) tuple nor a dict of them",
                    self.file,
                    self.line(other),
                    self.source(other)
                ),
            }
        }
        Ok(rows)
    }

    fn op(&self, tuple: &ast::ExprTuple, who: &str, target: &str) -> Result<LimitOp> {
        let at = format!(
            "{}:{}: {LIMITS_NAME} row for {who} on {target}",
            self.file,
            self.line(tuple)
        );
        let [op, argument] = tuple.elts.as_slice() else {
            bail!("{at} is {}, not (op, argument)", self.source(tuple));
        };
        let ast::Expr::Name(op) = op else {
            bail!(
                "{at} names its op as {}; the ops are {DIVIDE_OP} and {MAXIMUM_OP}",
                self.source(op)
            );
        };
        match (op.id.as_str(), number(argument)) {
            (DIVIDE_OP, Some(Number::Int(factor))) if factor > 0 => {
                Ok(LimitOp::Divide(DivideFactor::from_integer(factor)))
            }
            (DIVIDE_OP, Some(Number::Float(factor))) if factor.is_finite() && factor > 0.0 => {
                Ok(LimitOp::Divide(DivideFactor::from_float(factor)))
            }
            (MAXIMUM_OP, Some(Number::Int(maximum))) if maximum > 0 => {
                Ok(LimitOp::Maximum(maximum))
            }
            (DIVIDE_OP, _) => bail!(
                "{at} divides by {}; {DIVIDE_OP} needs a positive number",
                self.source(argument)
            ),
            (MAXIMUM_OP, _) => bail!(
                "{at} caps at {}; {MAXIMUM_OP} needs a positive integer",
                self.source(argument)
            ),
            (other, _) => bail!("{at} uses {other}; the ops are {DIVIDE_OP} and {MAXIMUM_OP}"),
        }
    }
}

fn offset(size: TextSize) -> usize {
    u32::from(size) as usize
}

fn line_at(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(text: &str) -> Vec<HookLimit> {
        parse_hooks(text, "eb_hooks.py")
            .unwrap_or_else(|error| panic!("{error:#}"))
            .limits
    }

    fn refusal(text: &str) -> String {
        format!("{:#}", parse_hooks(text, "eb_hooks.py").unwrap_err())
    }

    #[test]
    fn rows_resolve_constants_and_keep_the_literal_kind() {
        let text = "\
CPU_TARGET_A64FX = 'aarch64/a64fx'

def divide_by_factor(parallel, factor):
    return max(1, parallel // factor)

def set_maximum(parallel, max_value):
    return min(parallel, max_value)

PARALLELISM_LIMITS = {
    CPU_TARGET_A64FX: (divide_by_factor, 4),
    'libxc': {
        '*': (divide_by_factor, 2.5),
        CPU_TARGET_A64FX: (set_maximum, 6),
    },
}
";
        let rows = limits(text);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].software, None);
        assert_eq!(rows[0].target, "aarch64/a64fx");
        assert_eq!(rows[0].line, Some(10));
        let LimitOp::Divide(global) = rows[0].op else {
            panic!("{:?}", rows[0].op);
        };
        assert!(global.integer_literal());
        assert_eq!(global.value(), 4.0);
        assert_eq!(rows[1].software.as_deref(), Some("libxc"));
        assert_eq!(rows[1].target, "*");
        assert_eq!(rows[1].line, Some(12));
        let LimitOp::Divide(factor) = rows[1].op else {
            panic!("{:?}", rows[1].op);
        };
        assert!(!factor.integer_literal());
        assert_eq!(factor.value(), 2.5);
        assert_eq!(rows[2].target, "aarch64/a64fx");
        assert_eq!(rows[2].op, LimitOp::Maximum(6));
    }

    #[test]
    fn a_repeated_key_keeps_the_last_value_as_python_does() {
        let text = "\
PARALLELISM_LIMITS = {
    'libxc': {'*': (divide_by_factor, 2)},
    'LLVM': {'*': (divide_by_factor, 2), '*': (set_maximum, 8)},
    'libxc': {'x86_64/amd/zen4': (set_maximum, 4)},
}
";
        let rows = limits(text);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows
            .iter()
            .any(|row| row.software.as_deref() == Some("LLVM") && row.op == LimitOp::Maximum(8)));
        let libxc: Vec<&HookLimit> = rows
            .iter()
            .filter(|row| row.software.as_deref() == Some("libxc"))
            .collect();
        assert_eq!(libxc.len(), 1, "{rows:?}");
        assert_eq!(libxc[0].target, "x86_64/amd/zen4");
    }

    #[test]
    fn the_last_assignment_wins_and_uses_the_constants_bound_before_it() {
        let text = "\
TARGET = 'aarch64/a64fx'
PARALLELISM_LIMITS = {TARGET: (divide_by_factor, 4)}
TARGET = 'x86_64/amd/zen4'
PARALLELISM_LIMITS = {TARGET: (set_maximum, 8)}
";
        let rows = limits(text);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].target, "x86_64/amd/zen4");
        assert_eq!(rows[0].op, LimitOp::Maximum(8));
        assert_eq!(rows[0].line, Some(4));
    }

    #[test]
    fn rows_the_model_cannot_follow_are_refused_with_their_line() {
        let cases = [
            ("x = (\n", "eb_hooks.py:2: not valid Python"),
            ("LIMITS = {}\n", "eb_hooks.py: no module-level PARALLELISM_LIMITS"),
            (
                "PARALLELISM_LIMITS = dict()\n",
                "eb_hooks.py:1: PARALLELISM_LIMITS is not a dict literal",
            ),
            (
                "PARALLELISM_LIMITS = {\n    'libxc': {'*': (limit_by_memory, 6400)},\n}\n",
                "eb_hooks.py:2: PARALLELISM_LIMITS row for libxc on * uses limit_by_memory; the ops are divide_by_factor and set_maximum",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {'*': (set_maximum, 8.0)}}\n",
                "row for libxc on * caps at 8.0; set_maximum needs a positive integer",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {'*': (divide_by_factor, 0)}}\n",
                "row for libxc on * divides by 0; divide_by_factor needs a positive number",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {'*': (divide_by_factor, -2)}}\n",
                "row for libxc on * divides by -2; divide_by_factor needs a positive number",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {UNKNOWN: (set_maximum, 8)}}\n",
                "PARALLELISM_LIMITS key UNKNOWN is not a module-level string constant",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {'*': (set_maximum, 8, 1)}}\n",
                "row for libxc on * is (set_maximum, 8, 1), not (op, argument)",
            ),
            (
                "PARALLELISM_LIMITS = {'libxc': {}}\n",
                "PARALLELISM_LIMITS entry for libxc is empty",
            ),
            (
                "PARALLELISM_LIMITS = {'*': (divide_by_factor, 2)}\n",
                "needs a CPU target, not '*'",
            ),
            (
                "PARALLELISM_LIMITS = {**OTHER}\n",
                "PARALLELISM_LIMITS unpacks a dict with **",
            ),
        ];
        for (text, expected) in cases {
            let error = refusal(text);
            assert!(error.contains(expected), "{text:?}: {error}");
        }
    }
}
