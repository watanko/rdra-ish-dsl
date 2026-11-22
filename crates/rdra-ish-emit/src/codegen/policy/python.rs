//! Pure predicates keep business decisions separate from collection and I/O mechanisms.

use std::collections::BTreeMap;

use super::{build, Comparison, Expression, Field, Operand, Policy, Reference};
use crate::codegen::{actions::snake, string_literal, CodegenError, Lang, Options};
use rdra_ish_core::SemanticModel;

mod transitions;
use transitions::print_transitions;

type Result<T> = std::result::Result<T, CodegenError>;

pub(in crate::codegen) fn print_python(model: &SemanticModel, options: &Options) -> Result<String> {
    let policy = build(model, options);
    let sql = matches!(options.lang, Lang::PythonSqlPolicy);
    let mut out = crate::codegen::py::docstring(
        &[
            "Generated business decisions.".into(),
            format!("Source: {}.", options.model_path),
        ],
        "",
    );
    out.push_str("\nfrom __future__ import annotations\n\nfrom datetime import date, datetime\nfrom decimal import Decimal\nfrom typing import Literal\n\n");
    if sql {
        out.push_str("from sqlalchemy import and_, or_, not_, true, false, case\nfrom sqlalchemy.sql import ColumnElement, SQLColumnExpression\n\n");
    }
    for variation in model.variations.values() {
        let name = snake(&variation.id).to_uppercase();
        let values = variation
            .values
            .iter()
            .map(|value| string_literal(value))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "{name}Value = Literal[{values}]\n{name}: tuple[{name}Value, ...] = ({values},)\n"
        ));
        if variation.labels.iter().any(Option::is_some) {
            let labels = variation
                .values
                .iter()
                .zip(&variation.labels)
                .map(|(value, label)| {
                    format!(
                        "{}: {}",
                        string_literal(value),
                        string_literal(label.as_ref().unwrap_or(value))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "{name}_LABELS: dict[Literal[{values}], str] = {{{labels}}}\n"
            ));
        }
        out.push('\n');
    }
    for condition in &policy.conditions {
        if !condition.executable || !condition.quantifiers.is_empty() {
            return Err(unsupported(&condition.name));
        }
        let mut printer = Printer {
            policy: &policy,
            sql,
            arguments: BTreeMap::new(),
            visiting: Vec::new(),
        };
        let expressions = condition
            .comparisons
            .iter()
            .map(|value| printer.comparison(value))
            .collect::<Result<Vec<_>>>()?;
        let arguments = printer
            .arguments
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let signature = if arguments.is_empty() {
            String::new()
        } else {
            format!("*, {arguments}")
        };
        let expression = if expressions.is_empty() {
            if sql { "true()" } else { "True" }.into()
        } else if sql {
            format!("and_({})", expressions.join(", "))
        } else {
            expressions.join(" and ")
        };
        let result_type = if sql { "ColumnElement[bool]" } else { "bool" };
        out.push_str(&format!(
            "def {}({signature}) -> {result_type}:\n    {}\n    return {expression}\n\n\n",
            snake(&condition.name),
            string_literal(&condition.label)
        ));
    }
    print_transitions(&policy, sql, &mut out)?;
    Ok(out)
}

struct Printer<'a> {
    policy: &'a Policy,
    sql: bool,
    arguments: BTreeMap<String, String>,
    visiting: Vec<String>,
}

impl Printer<'_> {
    fn field(&mut self, reference: &Reference) -> Result<String> {
        let key = format!("{}.{}", reference.entity, reference.column);
        let field = self
            .policy
            .fields
            .iter()
            .find(|field| {
                field.reference.entity == reference.entity
                    && field.reference.column == reference.column
            })
            .ok_or_else(|| unsupported(&key))?;
        if field.unsupported_derivation.is_some() {
            return Err(unsupported(&format!("arithmetic derivation {key}")));
        }
        if let Some(expression) = &field.derived {
            if self.visiting.contains(&key) {
                return Err(unsupported(&format!("recursive derived field {key}")));
            }
            self.visiting.push(key);
            let result = self.expression(expression);
            self.visiting.pop();
            return result;
        }
        let name = format!("{}_{}", snake(&reference.entity), snake(&reference.column));
        let ty = field_type(field)?;
        let ty = if self.sql {
            format!(
                "SQLColumnExpression[{}]",
                if field.kind == "enum" { "str" } else { &ty }
            )
        } else {
            ty
        };
        self.arguments.insert(name.clone(), ty);
        Ok(name)
    }

    fn expression(&mut self, expression: &Expression) -> Result<String> {
        if let Some(comparison) = &expression.comparison {
            return self.comparison(comparison);
        }
        let children = expression
            .children
            .iter()
            .map(|child| self.expression(child))
            .collect::<Result<Vec<_>>>()?;
        if self.sql {
            return match expression.operator.as_str() {
                "not" if children.len() == 1 => Ok(format!("not_({})", children[0])),
                "and" | "or" if children.len() == 2 => {
                    Ok(format!("{}_({})", expression.operator, children.join(", ")))
                }
                _ => Err(unsupported(&expression.operator)),
            };
        }
        match expression.operator.as_str() {
            "not" if children.len() == 1 => Ok(format!("(not {})", children[0])),
            "and" | "or" if children.len() == 2 => Ok(format!(
                "({} {} {})",
                children[0], expression.operator, children[1]
            )),
            _ => Err(unsupported(&expression.operator)),
        }
    }

    fn operand(&mut self, operand: &Operand) -> Result<String> {
        match operand.kind.as_str() {
            "column" => self.field(
                operand
                    .reference
                    .as_ref()
                    .ok_or_else(|| unsupported("column without reference"))?,
            ),
            "integer" => operand
                .integer
                .map(|value| value.to_string())
                .ok_or_else(|| unsupported("missing integer")),
            "boolean" => operand
                .boolean
                .map(|value| if value { "True" } else { "False" }.into())
                .ok_or_else(|| unsupported("missing boolean")),
            "text" => operand
                .text
                .as_ref()
                .map(|value| string_literal(value))
                .ok_or_else(|| unsupported("missing text")),
            "null" => Ok("None".into()),
            "clock" => {
                self.arguments.insert(
                    "now".into(),
                    if self.sql {
                        "datetime | SQLColumnExpression[datetime]"
                    } else {
                        "datetime"
                    }
                    .into(),
                );
                Ok("now".into())
            }
            _ => Err(unsupported(&operand.kind)),
        }
    }

    fn comparison(&mut self, comparison: &Comparison) -> Result<String> {
        let left = self.field(&comparison.left)?;
        if self.sql {
            return self.sql_comparison(comparison, &left);
        }
        let mut right = Vec::new();
        for operand in &comparison.right {
            if operand.kind == "present" {
                return match comparison.operator.as_str() {
                    "==" => Ok(format!("({left} is not None)")),
                    "!=" => Ok(format!("({left} is None)")),
                    _ => Err(unsupported("ordered presence comparison")),
                };
            }
            right.push(self.operand(operand)?);
        }
        let operator = comparison.operator.as_str();
        if matches!(operator, "in" | "not in") {
            return Ok(format!("({left} {operator} ({},))", right.join(", ")));
        }
        if right.len() != 1 || !matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=") {
            return Err(unsupported(operator));
        }
        if right[0] == "None" {
            if !matches!(operator, "==" | "!=") {
                return Err(unsupported(operator));
            }
            return Ok(format!(
                "({left} {} None)",
                if operator == "==" { "is" } else { "is not" }
            ));
        }
        if let Some(boolean) = comparison.right[0].boolean {
            if matches!(operator, "==" | "!=") {
                if self.nullable(&comparison.left) {
                    return Ok(format!(
                        "({left} {} {})",
                        if operator == "==" { "is" } else { "is not" },
                        if boolean { "True" } else { "False" }
                    ));
                }
                let positive = boolean == (operator == "==");
                return Ok(if positive {
                    format!("({left})")
                } else {
                    format!("(not {left})")
                });
            }
        }
        if matches!(operator, "==" | "!=") {
            return Ok(format!("({left} {operator} {})", right[0]));
        }
        let mut guards = Vec::new();
        if self.nullable(&comparison.left) {
            guards.push(format!("{left} is not None"));
        }
        if let Some(reference) = &comparison.right[0].reference {
            if self.nullable(reference) {
                guards.push(format!("{} is not None", right[0]));
            }
        }
        guards.push(format!("{left} {operator} {}", right[0]));
        Ok(format!("({})", guards.join(" and ")))
    }

    fn sql_comparison(&mut self, comparison: &Comparison, left: &str) -> Result<String> {
        let op = comparison.operator.as_str();
        if comparison.right.len() == 1
            && matches!(comparison.right[0].kind.as_str(), "present" | "null")
        {
            let present = comparison.right[0].kind == "present";
            if !matches!(op, "==" | "!=") {
                return Err(unsupported(op));
            }
            return Ok(format!(
                "{left}.{}(None)",
                if present == (op == "==") {
                    "is_not"
                } else {
                    "is_"
                }
            ));
        }
        let right = comparison
            .right
            .iter()
            .map(|operand| self.operand(operand))
            .collect::<Result<Vec<_>>>()?;
        if matches!(op, "in" | "not in") {
            let equalities = right
                .iter()
                .map(|value| format!("{left}.is_not_distinct_from({value})"))
                .collect::<Vec<_>>();
            let included = if equalities.is_empty() {
                "false()".into()
            } else {
                format!("or_({})", equalities.join(", "))
            };
            return Ok(if op == "in" {
                included
            } else {
                format!("not_({included})")
            });
        }
        if right.len() != 1 {
            return Err(unsupported(op));
        }
        if comparison.right[0].boolean.is_some() {
            return Ok(format!(
                "{left}.{}({})",
                if op == "==" { "is_" } else { "is_not" },
                right[0]
            ));
        }
        if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
            return Err(unsupported(op));
        }
        if matches!(op, "==" | "!=") {
            return Ok(format!(
                "{left}.{}({})",
                if op == "==" {
                    "is_not_distinct_from"
                } else {
                    "is_distinct_from"
                },
                right[0]
            ));
        }
        let mut guards = Vec::new();
        if self.nullable(&comparison.left) {
            guards.push(format!("{left}.is_not(None)"));
        }
        if comparison.right[0]
            .reference
            .as_ref()
            .is_some_and(|reference| self.nullable(reference))
        {
            guards.push(format!("{}.is_not(None)", right[0]));
        }
        guards.push(format!("({left} {op} {})", right[0]));
        Ok(format!("and_({})", guards.join(", ")))
    }

    fn nullable(&self, reference: &Reference) -> bool {
        self.policy.fields.iter().any(|field| {
            field.reference.entity == reference.entity
                && field.reference.column == reference.column
                && field.nullable
        })
    }
}

fn field_type(field: &Field) -> Result<String> {
    let base = match field.kind.as_str() {
        "integer" => "int".into(),
        "decimal" => "Decimal".into(),
        "date" => "date".into(),
        "boolean" => "bool".into(),
        "datetime" => "datetime".into(),
        "string" => "str".into(),
        "enum" => format!(
            "Literal[{}]",
            field
                .values
                .iter()
                .map(|value| string_literal(value))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => return Err(unsupported(&field.kind)),
    };
    Ok(if field.nullable {
        format!("{base} | None")
    } else {
        base
    })
}

fn unsupported(value: &str) -> CodegenError {
    CodegenError::UnsupportedPolicy(value.into())
}
