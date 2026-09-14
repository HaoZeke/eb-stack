//! Static Python syntax model for Spack package directives.

use rustpython_parser::ast::{self, Constant, Ranged};
use rustpython_parser::Parse;
use std::collections::BTreeMap;

const DIRECTIVES: &[&str] = &[
    "conflicts",
    "depends_on",
    "license",
    "patch",
    "requires",
    "resource",
    "variant",
    "version",
];

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StaticValue {
    None,
    Bool(bool),
    String(String),
    Sequence(Vec<StaticValue>),
    Mapping(Vec<(String, StaticValue)>),
}

impl StaticValue {
    pub(crate) fn as_string(&self) -> Option<String> {
        match self {
            Self::String(value) => Some(value.clone()),
            Self::Bool(value) => Some(value.to_string()),
            Self::None | Self::Sequence(_) | Self::Mapping(_) => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn mapping_value(&self, key: &str) -> Option<&StaticValue> {
        match self {
            Self::Mapping(entries) => entries
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StaticCall {
    pub(crate) name: String,
    pub(crate) args: Vec<StaticValue>,
    pub(crate) kwargs: BTreeMap<String, StaticValue>,
    pub(crate) scoped_when: Vec<StaticScopedCondition>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StaticDynamicPatch {
    pub(crate) scoped_when: Vec<StaticScopedCondition>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StaticScopedCondition {
    Spec(String),
    Opaque(String),
}

impl StaticCall {
    pub(crate) fn arg_string(&self, index: usize) -> Option<String> {
        self.args.get(index)?.as_string()
    }

    pub(crate) fn kw_string(&self, name: &str) -> Option<String> {
        self.kwargs.get(name)?.as_string()
    }

    pub(crate) fn kw_bool(&self, name: &str) -> Option<bool> {
        self.kwargs.get(name)?.as_bool()
    }

    pub(crate) fn kw_value(&self, name: &str) -> Option<&StaticValue> {
        self.kwargs.get(name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpackSyntax {
    pub(crate) class_name: String,
    pub(crate) bases: Vec<String>,
    pub(crate) attributes: BTreeMap<String, StaticValue>,
    pub(crate) calls: Vec<StaticCall>,
    pub(crate) dynamic_patches: Vec<StaticDynamicPatch>,
    pub(crate) residuals: Vec<String>,
}

pub(crate) fn parse_spack_syntax(source: &str) -> Result<SpackSyntax, String> {
    let suite = ast::Suite::parse(source, "package.py").map_err(|error| error.to_string())?;
    let class = suite
        .iter()
        .find_map(|statement| match statement {
            ast::Stmt::ClassDef(class) => Some(class),
            _ => None,
        })
        .ok_or_else(|| "spack: no package class found".to_string())?;
    let bases = class
        .bases
        .iter()
        .filter_map(expression_name)
        .collect::<Vec<_>>();
    let mut evaluator = StaticEvaluator::new(source);
    evaluator.walk_statements(&class.body);
    let mut seen_residuals = std::collections::BTreeSet::new();
    evaluator
        .residuals
        .retain(|residual| seen_residuals.insert(residual.clone()));
    Ok(SpackSyntax {
        class_name: class.name.to_string(),
        bases,
        attributes: evaluator.attributes,
        calls: evaluator.calls,
        dynamic_patches: evaluator.dynamic_patches,
        residuals: evaluator.residuals,
    })
}

struct StaticEvaluator<'a> {
    source: &'a str,
    environment: BTreeMap<String, StaticValue>,
    attributes: BTreeMap<String, StaticValue>,
    calls: Vec<StaticCall>,
    dynamic_patches: Vec<StaticDynamicPatch>,
    residuals: Vec<String>,
    scoped_when: Vec<StaticScopedCondition>,
}

impl<'a> StaticEvaluator<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            environment: BTreeMap::new(),
            attributes: BTreeMap::new(),
            calls: Vec::new(),
            dynamic_patches: Vec::new(),
            residuals: Vec::new(),
            scoped_when: Vec::new(),
        }
    }

    fn walk_statements(&mut self, statements: &[ast::Stmt]) {
        for statement in statements {
            self.walk_statement(statement);
        }
    }

    fn walk_statement(&mut self, statement: &ast::Stmt) {
        match statement {
            ast::Stmt::Assign(assignment) => self.walk_assignment(assignment),
            ast::Stmt::AugAssign(assignment) => self.walk_aug_assignment(assignment),
            ast::Stmt::AnnAssign(assignment) => {
                if let Some(value) = assignment.value.as_deref() {
                    self.assign_expression(&assignment.target, value);
                }
            }
            ast::Stmt::Expr(statement) => {
                if let ast::Expr::Call(call) = statement.value.as_ref() {
                    self.record_directive(call);
                }
            }
            ast::Stmt::For(statement) => self.walk_for(statement),
            ast::Stmt::With(statement) => self.walk_with(statement),
            ast::Stmt::If(statement) => self.walk_if(statement),
            ast::Stmt::FunctionDef(function) => self.record_patch_method(function),
            ast::Stmt::AsyncFunctionDef(_)
            | ast::Stmt::ClassDef(_)
            | ast::Stmt::Import(_)
            | ast::Stmt::ImportFrom(_)
            | ast::Stmt::Pass(_) => {}
            _ if self.statement_mentions_directive(statement) => {
                self.residual(
                    statement,
                    "unsupported control flow contains package directives",
                );
            }
            _ => {}
        }
    }

    fn walk_aug_assignment(&mut self, assignment: &ast::StmtAugAssign) {
        let Some(left) = self.evaluate(&assignment.target) else {
            return;
        };
        let Some(right) = self.evaluate(&assignment.value) else {
            return;
        };
        let combined = match assignment.op {
            ast::Operator::Add => match (left, right) {
                (StaticValue::String(mut left), StaticValue::String(right)) => {
                    left.push_str(&right);
                    Some(StaticValue::String(left))
                }
                (StaticValue::Sequence(mut left), StaticValue::Sequence(right)) => {
                    left.extend(right);
                    Some(StaticValue::Sequence(left))
                }
                _ => None,
            },
            ast::Operator::Mod => self.evaluate_mod(left, right),
            _ => None,
        };
        if let Some(value) = combined {
            self.bind_target(&assignment.target, value.clone());
            if let ast::Expr::Name(name) = assignment.target.as_ref() {
                self.attributes.insert(name.id.to_string(), value);
            }
        } else {
            self.unbind_target(&assignment.target);
        }
    }

    fn evaluate_mod(&self, left: StaticValue, right: StaticValue) -> Option<StaticValue> {
        let template = left.as_string()?;
        let values = match right {
            StaticValue::Sequence(values) => values,
            value => vec![value],
        };
        let mut rendered = template;
        for value in values {
            rendered = replace_first(&rendered, "%s", &value.as_string()?)?;
        }
        if leftover_format_placeholder(&rendered) {
            return None;
        }
        Some(StaticValue::String(rendered))
    }

    fn walk_assignment(&mut self, assignment: &ast::StmtAssign) {
        let Some(value) = self.evaluate(&assignment.value) else {
            for target in &assignment.targets {
                self.unbind_target(target);
            }
            return;
        };
        for target in &assignment.targets {
            self.bind_target(target, value.clone());
            if let ast::Expr::Name(name) = target {
                self.attributes.insert(name.id.to_string(), value.clone());
            }
        }
    }

    fn assign_expression(&mut self, target: &ast::Expr, expression: &ast::Expr) {
        if let Some(value) = self.evaluate(expression) {
            self.bind_target(target, value.clone());
            if let ast::Expr::Name(name) = target {
                self.attributes.insert(name.id.to_string(), value);
            }
        } else {
            self.unbind_target(target);
        }
    }

    fn unbind_target(&mut self, target: &ast::Expr) {
        if let ast::Expr::Name(name) = target {
            self.environment.remove(name.id.as_str());
            self.attributes.remove(name.id.as_str());
        }
    }

    fn walk_for(&mut self, statement: &ast::StmtFor) {
        let Some(iterable) = self.evaluate(&statement.iter) else {
            if statement
                .body
                .iter()
                .any(|body| self.statement_mentions_directive(body))
            {
                self.residual(
                    statement.iter.as_ref(),
                    "dynamic for-loop contains package directives",
                );
            }
            return;
        };
        let Some(values) = iteration_values(iterable) else {
            self.residual(
                statement.iter.as_ref(),
                "non-iterable static for-loop value",
            );
            return;
        };
        if values.len() > 1024 {
            self.residual(
                statement.iter.as_ref(),
                "static for-loop exceeds 1024 values",
            );
            return;
        }
        if values.is_empty() {
            self.walk_statements(&statement.orelse);
            return;
        }
        for value in values {
            if self.bind_target(&statement.target, value) {
                self.walk_statements(&statement.body);
            } else {
                self.residual(
                    statement.target.as_ref(),
                    "for-loop target cannot bind static value",
                );
                return;
            }
        }
    }

    fn walk_with(&mut self, statement: &ast::StmtWith) {
        let mentions = statement
            .body
            .iter()
            .any(|body| self.statement_mentions_directive(body));
        let mut conditions = Vec::new();
        for item in &statement.items {
            let condition = match &item.context_expr {
                ast::Expr::Call(call)
                    if expression_name(&call.func).as_deref() == Some("when")
                        && call.args.len() == 1 =>
                {
                    match self
                        .evaluate(&call.args[0])
                        .and_then(|value| value.as_string())
                    {
                        Some(condition) => StaticScopedCondition::Spec(condition),
                        None => {
                            let source = self.source_fragment(&call.args[0]);
                            if mentions {
                                self.residual(
                                    &item.context_expr,
                                    &format!(
                                        "dynamic scoped when({source}) contains package directives"
                                    ),
                                );
                            }
                            StaticScopedCondition::Opaque(source)
                        }
                    }
                }
                _ => {
                    let source = self.source_fragment(&item.context_expr);
                    if mentions {
                        self.residual(
                            &item.context_expr,
                            "unsupported context contains package directives",
                        );
                    }
                    StaticScopedCondition::Opaque(source)
                }
            };
            conditions.push(condition);
        }
        let original_len = self.scoped_when.len();
        self.scoped_when.extend(conditions);
        self.walk_statements(&statement.body);
        self.scoped_when.truncate(original_len);
    }

    fn walk_if(&mut self, statement: &ast::StmtIf) {
        match self
            .evaluate(&statement.test)
            .and_then(|value| is_truthy(&value))
        {
            Some(true) => self.walk_statements(&statement.body),
            Some(false) => self.walk_statements(&statement.orelse),
            None if statement
                .body
                .iter()
                .chain(&statement.orelse)
                .any(|body| self.statement_mentions_directive(body)) =>
            {
                self.residual(
                    statement.test.as_ref(),
                    "dynamic if-statement contains package directives",
                );
            }
            None => {}
        }
    }

    fn record_directive(&mut self, call: &ast::ExprCall) {
        let Some(name) = expression_name(&call.func) else {
            return;
        };
        if !DIRECTIVES.contains(&name.as_str()) {
            return;
        }
        let args = call
            .args
            .iter()
            .map(|argument| self.evaluate(argument))
            .collect::<Option<Vec<_>>>();
        let kwargs = call
            .keywords
            .iter()
            .map(|keyword| {
                let name = keyword.arg.as_ref()?.to_string();
                Some((name, self.evaluate(&keyword.value)?))
            })
            .collect::<Option<BTreeMap<_, _>>>();
        let (Some(args), Some(kwargs)) = (args, kwargs) else {
            self.residual(call, &format!("dynamic {name}() directive"));
            return;
        };
        let range = call.range();
        self.calls.push(StaticCall {
            name,
            args,
            kwargs,
            scoped_when: self.scoped_when.clone(),
            start: text_offset(range.start()),
            end: text_offset(range.end()),
        });
    }

    fn record_patch_method(&mut self, function: &ast::StmtFunctionDef) {
        if function.name.as_str() != "patch" {
            return;
        }
        let mut scoped_when = self.scoped_when.clone();
        for decorator in &function.decorator_list {
            let condition = match decorator {
                ast::Expr::Call(call)
                    if expression_name(&call.func).as_deref() == Some("when")
                        && call.args.len() == 1 =>
                {
                    match self
                        .evaluate(&call.args[0])
                        .and_then(|value| value.as_string())
                    {
                        Some(condition) => StaticScopedCondition::Spec(condition),
                        None => StaticScopedCondition::Opaque(self.source_fragment(&call.args[0])),
                    }
                }
                _ => continue,
            };
            scoped_when.push(condition);
        }
        let range = function.range();
        self.dynamic_patches.push(StaticDynamicPatch {
            scoped_when,
            start: text_offset(range.start()),
            end: text_offset(range.end()),
        });
    }

    fn evaluate(&self, expression: &ast::Expr) -> Option<StaticValue> {
        match expression {
            ast::Expr::Constant(constant) => match &constant.value {
                Constant::None => Some(StaticValue::None),
                Constant::Bool(value) => Some(StaticValue::Bool(*value)),
                Constant::Str(value) => Some(StaticValue::String(value.clone())),
                Constant::Int(value) => Some(StaticValue::String(value.to_string())),
                _ => None,
            },
            ast::Expr::Name(name) => self.environment.get(name.id.as_str()).cloned(),
            ast::Expr::Tuple(tuple) => self.evaluate_sequence(&tuple.elts),
            ast::Expr::List(list) => self.evaluate_sequence(&list.elts),
            ast::Expr::Set(set) => self.evaluate_sequence(&set.elts),
            ast::Expr::Dict(dictionary) => self.evaluate_mapping(dictionary),
            ast::Expr::Call(call) => self.evaluate_call(call),
            ast::Expr::BinOp(binary) => self.evaluate_binary(binary),
            ast::Expr::JoinedStr(joined) => self.evaluate_joined_string(joined),
            ast::Expr::FormattedValue(formatted) => self
                .evaluate(&formatted.value)
                .and_then(|value| value.as_string())
                .map(StaticValue::String),
            ast::Expr::Subscript(subscript) => self.evaluate_subscript(subscript),
            ast::Expr::BoolOp(boolean) => self.evaluate_bool_op(boolean),
            ast::Expr::Compare(compare) => self.evaluate_comparison(compare),
            ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => self
                .evaluate(&unary.operand)
                .and_then(|value| value.as_bool())
                .map(|value| StaticValue::Bool(!value)),
            ast::Expr::IfExp(conditional) => {
                match self
                    .evaluate(&conditional.test)
                    .and_then(|value| is_truthy(&value))
                {
                    Some(true) => self.evaluate(&conditional.body),
                    Some(false) => self.evaluate(&conditional.orelse),
                    None => None,
                }
            }
            _ => None,
        }
    }

    fn evaluate_sequence(&self, expressions: &[ast::Expr]) -> Option<StaticValue> {
        expressions
            .iter()
            .map(|expression| self.evaluate(expression))
            .collect::<Option<Vec<_>>>()
            .map(StaticValue::Sequence)
    }

    fn evaluate_mapping(&self, dictionary: &ast::ExprDict) -> Option<StaticValue> {
        dictionary
            .keys
            .iter()
            .zip(&dictionary.values)
            .map(|(key, value)| {
                let key = self.evaluate(key.as_ref()?)?.as_string()?;
                Some((key, self.evaluate(value)?))
            })
            .collect::<Option<Vec<_>>>()
            .map(StaticValue::Mapping)
    }

    fn evaluate_call(&self, call: &ast::ExprCall) -> Option<StaticValue> {
        if let ast::Expr::Attribute(attribute) = call.func.as_ref() {
            let receiver = self.evaluate(&attribute.value)?;
            return match attribute.attr.as_str() {
                "get" => {
                    let StaticValue::Mapping(_) = &receiver else {
                        return None;
                    };
                    let key = self.evaluate(call.args.first()?)?.as_string()?;
                    if let Some(value) = receiver.mapping_value(&key).cloned() {
                        return Some(value);
                    }
                    match call.args.get(1) {
                        None => Some(StaticValue::None),
                        Some(default) => self.evaluate(default),
                    }
                }
                "items" if call.args.is_empty() => match receiver {
                    StaticValue::Mapping(entries) => Some(StaticValue::Sequence(
                        entries
                            .into_iter()
                            .map(|(key, value)| {
                                StaticValue::Sequence(vec![StaticValue::String(key), value])
                            })
                            .collect(),
                    )),
                    _ => None,
                },
                "keys" if call.args.is_empty() => match receiver {
                    StaticValue::Mapping(entries) => Some(StaticValue::Sequence(
                        entries
                            .into_iter()
                            .map(|(key, _)| StaticValue::String(key))
                            .collect(),
                    )),
                    _ => None,
                },
                "format" => {
                    let mut template = receiver.as_string()?;
                    for (index, argument) in call.args.iter().enumerate() {
                        let value = self.evaluate(argument)?.as_string()?;
                        let numbered = format!("{{{index}}}");
                        template = replace_first(&template, "{}", &value)
                            .or_else(|| replace_first(&template, &numbered, &value))?;
                    }
                    for keyword in &call.keywords {
                        let Some(name) = keyword.arg.as_ref() else {
                            continue;
                        };
                        let value = self.evaluate(&keyword.value)?.as_string()?;
                        let named = format!("{{{name}}}");
                        template = replace_first(&template, &named, &value)?;
                    }
                    if leftover_format_placeholder(&template) {
                        return None;
                    }
                    Some(StaticValue::String(template))
                }
                "replace" if call.args.len() == 2 => {
                    let input = receiver.as_string()?;
                    let from = self.evaluate(&call.args[0])?.as_string()?;
                    let to = self.evaluate(&call.args[1])?.as_string()?;
                    Some(StaticValue::String(input.replace(&from, &to)))
                }
                _ => None,
            };
        }
        match expression_name(&call.func).as_deref() {
            Some("str") if call.args.len() == 1 => self
                .evaluate(&call.args[0])?
                .as_string()
                .map(StaticValue::String),
            _ => None,
        }
    }

    fn evaluate_binary(&self, binary: &ast::ExprBinOp) -> Option<StaticValue> {
        let left = self.evaluate(&binary.left)?;
        let right = self.evaluate(&binary.right)?;
        match binary.op {
            ast::Operator::Add => match (left, right) {
                (StaticValue::String(mut left), StaticValue::String(right)) => {
                    left.push_str(&right);
                    Some(StaticValue::String(left))
                }
                (StaticValue::Sequence(mut left), StaticValue::Sequence(right)) => {
                    left.extend(right);
                    Some(StaticValue::Sequence(left))
                }
                _ => None,
            },
            ast::Operator::Mod => self.evaluate_mod(left, right),
            _ => None,
        }
    }

    fn evaluate_joined_string(&self, joined: &ast::ExprJoinedStr) -> Option<StaticValue> {
        let mut rendered = String::new();
        for value in &joined.values {
            rendered.push_str(&self.evaluate(value)?.as_string()?);
        }
        Some(StaticValue::String(rendered))
    }

    fn evaluate_subscript(&self, subscript: &ast::ExprSubscript) -> Option<StaticValue> {
        let container = self.evaluate(&subscript.value)?;
        let key = self.evaluate(&subscript.slice)?.as_string()?;
        match container {
            StaticValue::Mapping(entries) => entries
                .into_iter()
                .find(|(candidate, _)| candidate == &key)
                .map(|(_, value)| value),
            StaticValue::Sequence(values) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index).cloned()),
            _ => None,
        }
    }

    fn evaluate_bool_op(&self, boolean: &ast::ExprBoolOp) -> Option<StaticValue> {
        match boolean.op {
            ast::BoolOp::And => {
                let mut last = StaticValue::Bool(true);
                for value in &boolean.values {
                    last = self.evaluate(value)?;
                    if !is_truthy(&last)? {
                        return Some(StaticValue::Bool(false));
                    }
                }
                Some(last)
            }
            ast::BoolOp::Or => {
                for value in &boolean.values {
                    let evaluated = self.evaluate(value)?;
                    if is_truthy(&evaluated)? {
                        return Some(evaluated);
                    }
                }
                Some(StaticValue::Bool(false))
            }
        }
    }

    fn evaluate_comparison(&self, comparison: &ast::ExprCompare) -> Option<StaticValue> {
        if comparison.ops.len() != 1 || comparison.comparators.len() != 1 {
            return None;
        }
        let left = self.evaluate(&comparison.left)?;
        let right = self.evaluate(&comparison.comparators[0])?;
        let result = match comparison.ops[0] {
            ast::CmpOp::Eq | ast::CmpOp::Is => left == right,
            ast::CmpOp::NotEq | ast::CmpOp::IsNot => left != right,
            ast::CmpOp::In | ast::CmpOp::NotIn => {
                let contained = contains_value(&right, &left);
                if comparison.ops[0] == ast::CmpOp::In {
                    contained
                } else {
                    !contained
                }
            }
            _ => return None,
        };
        Some(StaticValue::Bool(result))
    }

    fn bind_target(&mut self, target: &ast::Expr, value: StaticValue) -> bool {
        match target {
            ast::Expr::Name(name) => {
                self.environment.insert(name.id.to_string(), value);
                true
            }
            ast::Expr::Tuple(tuple) => bind_sequence(self, &tuple.elts, value),
            ast::Expr::List(list) => bind_sequence(self, &list.elts, value),
            _ => false,
        }
    }

    fn statement_mentions_directive(&self, statement: &ast::Stmt) -> bool {
        let range = statement.range();
        self.source
            .get(text_offset(range.start())..text_offset(range.end()))
            .is_some_and(text_mentions_directive)
    }

    fn residual(&mut self, node: &impl Ranged, summary: &str) {
        let range = node.range();
        let line = self.source[..text_offset(range.start())]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        self.residuals
            .push(format!("residual: {summary} at package.py:{line}"));
    }

    fn source_fragment(&self, node: &impl Ranged) -> String {
        let range = node.range();
        self.source
            .get(text_offset(range.start())..text_offset(range.end()))
            .unwrap_or("<unknown>")
            .trim()
            .to_string()
    }
}

fn bind_sequence(
    evaluator: &mut StaticEvaluator<'_>,
    targets: &[ast::Expr],
    value: StaticValue,
) -> bool {
    let StaticValue::Sequence(values) = value else {
        return false;
    };
    targets.len() == values.len()
        && targets
            .iter()
            .zip(values)
            .all(|(target, value)| evaluator.bind_target(target, value))
}

fn iteration_values(value: StaticValue) -> Option<Vec<StaticValue>> {
    match value {
        StaticValue::Sequence(values) => Some(values),
        StaticValue::Mapping(entries) => Some(
            entries
                .into_iter()
                .map(|(key, _)| StaticValue::String(key))
                .collect(),
        ),
        _ => None,
    }
}

fn expression_name(expression: &ast::Expr) -> Option<String> {
    match expression {
        ast::Expr::Name(name) => Some(name.id.to_string()),
        ast::Expr::Attribute(attribute) => Some(attribute.attr.to_string()),
        _ => None,
    }
}

fn contains_value(container: &StaticValue, needle: &StaticValue) -> bool {
    match container {
        StaticValue::Sequence(values) => values.contains(needle),
        StaticValue::Mapping(entries) => needle
            .as_string()
            .is_some_and(|needle| entries.iter().any(|(key, _)| key == &needle)),
        StaticValue::String(value) => needle
            .as_string()
            .is_some_and(|needle| value.contains(&needle)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_numbered_and_named_fields() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    ver = "1.2"
    version("1.2", url="https://example.invalid/{0}.tar.gz".format(ver))
    version("1.3", url="https://example.invalid/{name}.tar.gz".format(name=ver))
"#,
        )
        .expect("parse");
        let urls: Vec<_> = syntax
            .calls
            .iter()
            .filter_map(|call| call.kw_string("url"))
            .collect();
        assert_eq!(
            urls,
            [
                "https://example.invalid/1.2.tar.gz",
                "https://example.invalid/1.2.tar.gz"
            ]
        );
    }

    #[test]
    fn leftover_format_is_not_a_fact() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    version("1.2", url="https://example.invalid/{0}-{1}.tar.gz".format("only"))
"#,
        )
        .expect("parse");
        assert!(
            syntax
                .residuals
                .iter()
                .any(|residual| residual.contains("dynamic version")),
            "leftover placeholder must residual, not become a source URL: {:?}",
            syntax.residuals
        );
    }

    #[test]
    fn augassign_extends_a_static_version_list() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    versions = ["1.0"]
    versions += ["2.0"]
    for v in versions:
        version(v, sha256="aaa")
"#,
        )
        .expect("parse");
        let versions: Vec<_> = syntax
            .calls
            .iter()
            .filter(|call| call.name == "version")
            .filter_map(|call| call.arg_string(0))
            .collect();
        assert_eq!(versions, ["1.0", "2.0"]);
    }

    #[test]
    fn bool_or_short_circuits_past_a_dynamic_operand() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    if True or discover():
        depends_on("backend")
"#,
        )
        .expect("parse");
        assert!(
            syntax.calls.iter().any(|call| call.name == "depends_on"
                && call.arg_string(0).as_deref() == Some("backend")),
            "static True or dynamic must keep the body: {:?}",
            syntax.residuals
        );
    }

    #[test]
    fn dict_get_with_a_dynamic_default_is_not_none() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    options = {}
    variant("feature", when=options.get("when", discover_when()))
"#,
        )
        .expect("parse");
        assert!(
            syntax
                .residuals
                .iter()
                .any(|residual| residual.contains("dynamic variant")),
            "failed default must residual, not become Always: {:?}",
            syntax.residuals
        );
    }

    #[test]
    fn url_for_version_is_not_a_version_directive() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    if runtime:
        def url_for_version(self, version):
            return "https://example.invalid"
"#,
        )
        .expect("parse");
        assert!(
            syntax.residuals.is_empty(),
            "url_for_version must not look like version(: {:?}",
            syntax.residuals
        );
    }

    #[test]
    fn a_failed_rebind_forgets_the_previous_value() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    ver = "1.0"
    ver = discover_revision()
    version(ver, sha256="aaa")
"#,
        )
        .expect("parse");
        assert!(
            syntax
                .residuals
                .iter()
                .any(|residual| residual.contains("dynamic version")),
            "stale ver must not become version(1.0): calls={:?} residuals={:?}",
            syntax.calls,
            syntax.residuals
        );
    }

    #[test]
    fn a_directive_free_with_does_not_residual() {
        let syntax = parse_spack_syntax(
            r#"
class Pkg(Package):
    with open("notes.txt") as handle:
        homepage = "https://example.invalid"
"#,
        )
        .expect("parse");
        assert!(
            syntax.residuals.is_empty(),
            "directive-free with must stay quiet: {:?}",
            syntax.residuals
        );
    }
}

fn is_truthy(value: &StaticValue) -> Option<bool> {
    match value {
        StaticValue::Bool(value) => Some(*value),
        StaticValue::None => Some(false),
        StaticValue::String(value) => Some(!value.is_empty()),
        StaticValue::Sequence(value) => Some(!value.is_empty()),
        StaticValue::Mapping(value) => Some(!value.is_empty()),
    }
}

fn text_mentions_directive(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        if line.starts_with('#') {
            return false;
        }
        DIRECTIVES.iter().any(|directive| {
            let needle = format!("{directive}(");
            let mut from = 0;
            while let Some(index) = line[from..].find(&needle) {
                let at = from + index;
                let before = line[..at].chars().next_back();
                if before
                    .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
                {
                    return true;
                }
                from = at + 1;
            }
            false
        })
    })
}

fn leftover_format_placeholder(value: &str) -> bool {
    value.contains("{") || value.contains("%s") || value.contains("%d")
}

fn replace_first(input: &str, pattern: &str, replacement: &str) -> Option<String> {
    let index = input.find(pattern)?;
    let mut output = String::with_capacity(input.len() + replacement.len());
    output.push_str(&input[..index]);
    output.push_str(replacement);
    output.push_str(&input[index + pattern.len()..]);
    Some(output)
}

fn text_offset(offset: rustpython_parser::text_size::TextSize) -> usize {
    u32::from(offset) as usize
}
