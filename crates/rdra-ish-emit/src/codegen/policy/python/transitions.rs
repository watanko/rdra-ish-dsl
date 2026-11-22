//! State decisions share the conditions used by the imperative and SQL printers.

use super::{snake, string_literal, unsupported, Policy, Printer, Result};
use std::collections::BTreeMap;

pub(super) fn print_transitions(policy: &Policy, sql: bool, out: &mut String) -> Result<()> {
    let mut groups = BTreeMap::new();
    for transition in &policy.transitions {
        groups
            .entry((
                transition.usecase.clone(),
                transition.target.entity.clone(),
                transition.target.column.clone(),
            ))
            .or_insert_with(Vec::new)
            .push(transition);
    }
    for ((usecase, _, column), transitions) in groups {
        let mut targets_by_source = BTreeMap::new();
        for transition in &transitions {
            if let Some(previous) = targets_by_source.insert(&transition.from, &transition.to) {
                if previous != &transition.to {
                    return Err(unsupported(&format!(
                        "{usecase}: multiple destinations from {}",
                        transition.from
                    )));
                }
            }
        }
        let mut printer = Printer {
            policy,
            sql,
            arguments: BTreeMap::new(),
            visiting: Vec::new(),
        };
        let state = printer.field(&transitions[0].target)?;
        let mut branches = Vec::new();
        let mut destinations = Vec::new();
        let mut target_groups = BTreeMap::new();
        for transition in transitions {
            target_groups
                .entry((transition.to.clone(), transition.guard.clone()))
                .or_insert_with(Vec::new)
                .push(transition.from.clone());
        }
        for ((to, guard), sources) in target_groups {
            let values = sources
                .iter()
                .map(|source| string_literal(source))
                .collect::<Vec<_>>()
                .join(", ");
            let mut conditions = vec![if sql {
                format!("{state}.in_(({values},))")
            } else {
                format!("({state} in ({values},))")
            }];
            let guard_names = policy
                .guards
                .iter()
                .filter(|guard| guard.usecase == usecase)
                .map(|guard| &guard.condition)
                .chain(guard.iter());
            for name in guard_names {
                let condition = policy
                    .conditions
                    .iter()
                    .find(|condition| &condition.name == name)
                    .ok_or_else(|| unsupported(name))?;
                if !condition.executable || !condition.quantifiers.is_empty() {
                    return Err(unsupported(name));
                }
                for comparison in &condition.comparisons {
                    conditions.push(printer.comparison(comparison)?);
                }
            }
            let expression = if sql {
                format!("and_({})", conditions.join(", "))
            } else {
                conditions.join(" and ")
            };
            let destination = string_literal(&to);
            if !destinations.contains(&destination) {
                destinations.push(destination.clone());
            }
            branches.push((expression, destination));
        }
        let arguments = printer
            .arguments
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", ");
        let result_type = if sql {
            "ColumnElement[str]".into()
        } else {
            format!("Literal[{}]", destinations.join(", "))
        };
        out.push_str(&format!("def decide_{}_{}(*, {arguments}) -> {result_type}:\n    \"\"\"Apply the modeled {usecase} state transition.\"\"\"\n", snake(&usecase), snake(&column)));
        if sql {
            let cases = branches
                .iter()
                .map(|(condition, destination)| format!("({condition}, {destination})"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("    return case({cases})\n\n\n"));
        } else {
            for (condition, destination) in branches {
                out.push_str(&format!(
                    "    if {condition}:\n        return {destination}\n"
                ));
            }
            out.push_str(&format!(
                "    raise ValueError({})\n\n\n",
                string_literal(&format!("{usecase}: no enabled transition"))
            ));
        }
    }
    Ok(())
}
