//! Column effects select values; adapters retain clocks, writes, and external effects.

use super::{
    field_base_type, field_type, snake, string_literal, unsupported, Policy, Printer, Result,
};
use std::collections::BTreeMap;

pub(super) fn print_effects(policy: &Policy, sql: bool, out: &mut String) -> Result<()> {
    let mut groups = BTreeMap::new();
    for effect in &policy.effects {
        groups
            .entry((
                effect.usecase.as_str(),
                effect.target.entity.as_str(),
                effect.target.column.as_str(),
            ))
            .or_insert_with(Vec::new)
            .push(effect);
    }
    for ((usecase, entity, column), effects) in groups {
        if policy.transitions.iter().any(|transition| {
            transition.usecase == usecase
                && transition.target.entity == entity
                && transition.target.column == column
        }) {
            return Err(unsupported(&format!(
                "{usecase}: state transition and column effect share {entity}.{column}"
            )));
        }
        for (index, first) in effects.iter().enumerate() {
            for second in effects.iter().skip(index + 1) {
                if first.value != second.value {
                    super::guards::ensure_disjoint(
                        policy,
                        usecase,
                        first.guard.as_deref(),
                        second.guard.as_deref(),
                    )?;
                }
            }
        }
        let target = &effects[0].target;
        let field = policy
            .fields
            .iter()
            .find(|field| &field.reference == target)
            .ok_or_else(|| unsupported("effect target is missing"))?;
        let mut printer = Printer {
            policy,
            sql,
            arguments: BTreeMap::new(),
            visiting: Vec::new(),
        };
        let mut branches = Vec::new();
        for effect in effects {
            let mut conditions = Vec::new();
            for name in policy
                .guards
                .iter()
                .filter(|guard| guard.usecase == usecase)
                .map(|guard| guard.condition.as_str())
                .chain(effect.guard.as_deref())
            {
                conditions.push(printer.condition_call(name)?);
            }
            let condition = if conditions.is_empty() {
                if sql { "true()" } else { "True" }.into()
            } else if sql {
                format!("and_({})", conditions.join(", "))
            } else {
                conditions.join(" and ")
            };
            if matches!(field.kind.as_str(), "date" | "datetime") && effect.value.kind == "integer"
            {
                return Err(unsupported(
                    "abstract clock marks need a concrete supplied time",
                ));
            }
            let value = if effect.value.kind == "present" {
                let name = format!("new_{}_{}", snake(entity), snake(column));
                let ty = if sql {
                    let base = if field.kind == "enum" {
                        "str".into()
                    } else {
                        field_base_type(field)?
                    };
                    format!("SQLColumnExpression[{base}]")
                } else {
                    field_base_type(field)?
                };
                printer.arguments.insert(name.clone(), ty);
                name
            } else {
                printer.operand(&effect.value)?
            };
            branches.push((condition, value));
        }
        let args = printer
            .arguments
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let signature = if args.is_empty() {
            String::new()
        } else {
            format!("*, {args}")
        };
        let result = if sql {
            format!(
                "ColumnElement[{}]",
                if field.kind == "enum" {
                    if field.nullable {
                        "str | None".into()
                    } else {
                        "str".into()
                    }
                } else {
                    field_type(field)?
                }
            )
        } else {
            field_type(field)?
        };
        out.push_str(&format!("def decide_{}_{}({signature}) -> {result}:\n    \"\"\"Apply the modeled {usecase} column effect.\"\"\"\n", snake(usecase), snake(column)));
        if sql {
            let cases = branches
                .iter()
                .map(|(condition, value)| format!("({condition}, {value})"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("    return case({cases})\n\n\n"));
        } else {
            let mut unconditional = false;
            for (condition, value) in branches {
                if condition == "True" {
                    out.push_str(&format!("    return {value}\n\n\n"));
                    unconditional = true;
                    break;
                }
                out.push_str(&format!("    if {condition}:\n        return {value}\n"));
            }
            // An unconditional effect returns above, so only conditional groups can reject.
            if !unconditional {
                out.push_str(&format!(
                    "    raise ValueError({})\n\n\n",
                    string_literal(&format!("{usecase}: no enabled column effect"))
                ));
            }
        }
    }
    Ok(())
}
