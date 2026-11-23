//! Deterministic code requires mutually exclusive outcomes, never branch priority.

use super::{unsupported, Comparison, Expression, Operand, Policy, Result};

type Conjunction = Vec<Comparison>;
type Alternatives = Vec<Conjunction>;

pub(super) fn ensure_disjoint(
    policy: &Policy,
    usecase: &str,
    left: Option<&str>,
    right: Option<&str>,
) -> Result<()> {
    let first = alternatives(policy, usecase, left)?;
    let second = alternatives(policy, usecase, right)?;
    if first
        .iter()
        .all(|a| second.iter().all(|b| contradict(a, b)))
    {
        Ok(())
    } else {
        Err(unsupported(&format!(
            "{usecase}: multiple destinations require provably disjoint guards"
        )))
    }
}

fn alternatives(policy: &Policy, usecase: &str, arm: Option<&str>) -> Result<Alternatives> {
    let mut result = vec![Vec::new()];
    let names = policy
        .guards
        .iter()
        .filter(|guard| guard.usecase == usecase)
        .map(|guard| guard.condition.as_str())
        .chain(arm);
    for name in names {
        let condition = policy
            .conditions
            .iter()
            .find(|value| value.name == name)
            .ok_or_else(|| unsupported(name))?;
        if !condition.executable || !condition.quantifiers.is_empty() {
            return Err(unsupported(name));
        }
        for comparison in &condition.comparisons {
            result = product(&result, &expand_comparison(policy, comparison, false)?);
        }
    }
    Ok(result)
}

fn expand_comparison(policy: &Policy, value: &Comparison, negative: bool) -> Result<Alternatives> {
    let derived = policy
        .fields
        .iter()
        .find(|field| field.reference == value.left)
        .and_then(|field| field.derived.as_ref());
    if let (Some(expression), [operand]) = (derived, value.right.as_slice()) {
        if let Some(boolean) = operand.boolean {
            if matches!(value.operator.as_str(), "==" | "!=") {
                let invert = negative ^ !boolean ^ (value.operator == "!=");
                return expand_expression(policy, expression, invert);
            }
        }
    }
    let mut atom = value.clone();
    if negative {
        atom.operator = inverse(&atom.operator)?.into();
    }
    if atom.right.len() == 1 && atom.right[0].kind == "present" {
        atom.operator = inverse(&atom.operator)?.into();
        atom.right = vec![Operand {
            kind: "null".into(),
            ..Operand::default()
        }];
    }
    Ok(vec![vec![atom]])
}

fn expand_expression(policy: &Policy, value: &Expression, negative: bool) -> Result<Alternatives> {
    if let Some(comparison) = &value.comparison {
        return expand_comparison(policy, comparison, negative);
    }
    if value.operator == "not" && value.children.len() == 1 {
        return expand_expression(policy, &value.children[0], !negative);
    }
    if value.children.len() != 2 || !matches!(value.operator.as_str(), "and" | "or") {
        return Err(unsupported("non-boolean guard expression"));
    }
    let first = expand_expression(policy, &value.children[0], negative)?;
    let second = expand_expression(policy, &value.children[1], negative)?;
    Ok(if (value.operator == "and") ^ negative {
        product(&first, &second)
    } else {
        first.into_iter().chain(second).collect()
    })
}

fn product(first: &Alternatives, second: &Alternatives) -> Alternatives {
    first
        .iter()
        .flat_map(|left| {
            second
                .iter()
                .map(move |right| left.iter().chain(right).cloned().collect())
        })
        .collect()
}

fn contradict(first: &Conjunction, second: &Conjunction) -> bool {
    first.iter().any(|a| {
        second.iter().any(|b| {
            if a.left != b.left {
                return false;
            }
            if a.right == b.right && inverse(&a.operator).is_ok_and(|op| op == b.operator) {
                return true;
            }
            a.operator == "=="
                && b.operator == "=="
                && a.right.len() == 1
                && b.right.len() == 1
                && a.right != b.right
                && matches!(
                    a.right[0].kind.as_str(),
                    "text" | "boolean" | "integer" | "null"
                )
                && matches!(
                    b.right[0].kind.as_str(),
                    "text" | "boolean" | "integer" | "null"
                )
        })
    })
}

fn inverse(operator: &str) -> Result<&'static str> {
    match operator {
        "==" => Ok("!="),
        "!=" => Ok("=="),
        "<" => Ok(">="),
        "<=" => Ok(">"),
        ">" => Ok("<="),
        ">=" => Ok("<"),
        "in" => Ok("not in"),
        "not in" => Ok("in"),
        _ => Err(unsupported(operator)),
    }
}
