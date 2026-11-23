//! State decisions share the conditions used by the imperative and SQL printers.

use super::{snake, string_literal, Policy, Printer, Result};
use std::collections::{BTreeMap, BTreeSet};

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
        for (index, left) in transitions.iter().enumerate() {
            for right in transitions.iter().skip(index + 1) {
                if left.from == right.from && left.to != right.to {
                    super::guards::ensure_disjoint(
                        policy,
                        &usecase,
                        left.guard.as_deref(),
                        right.guard.as_deref(),
                    )?;
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
        let global_conditions = policy
            .guards
            .iter()
            .filter(|guard| guard.usecase == usecase)
            .map(|guard| printer.condition_call(&guard.condition))
            .collect::<Result<Vec<_>>>()?;
        let source_values = transitions
            .iter()
            .map(|transition| string_literal(&transition.from))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
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
            if sql {
                conditions.extend(global_conditions.iter().cloned());
            }
            if let Some(name) = &guard {
                conditions.push(printer.condition_call(name)?);
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
            // Shared pure guards need one check, while invalid source states still reject first.
            if !global_conditions.is_empty() {
                let condition = format!(
                    "({state} in ({source_values},)) and {}",
                    global_conditions.join(" and ")
                );
                out.push_str(&format!(
                    "    if not ({condition}):\n        raise ValueError({})\n",
                    string_literal(&format!("{usecase}: no enabled transition"))
                ));
            }
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
