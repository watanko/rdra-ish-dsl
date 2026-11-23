//! Formatting of SQL identifiers, annotations, and review notes.

use super::*;

pub(super) fn local_text(conds: &[EntityCondition]) -> String {
    EntityCondition::equals(conds)
        .map(|(c, v)| format!("{c} == {}", v.literal()))
        .chain(EntityCondition::comparisons(conds).map(ComparisonProp::display))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn inv_text(inv: &Invariant) -> String {
    use rdra_ish_core::lifecycle::InvText;
    match &inv.text {
        InvText::Forbidden(c)
        | InvText::Required(c)
        | InvText::Exclusive(c)
        | InvText::Check(c) => c.clone(),
        InvText::Invariant { guards, requireds } => format!("when {guards} then {requireds}"),
        InvText::CrossForbidden {
            entities,
            conditions,
        } => format!("{entities}: {conditions}"),
        InvText::CrossInvariant {
            entities,
            guards,
            requireds,
        } => format!("{entities}: when {guards} then {requireds}"),
        InvText::Quantifier {
            anchor, constraint, ..
        } => format!("{anchor}: {constraint}"),
        InvText::Unique {
            child,
            parent,
            role,
        }
        | InvText::MinCard {
            child,
            parent,
            role,
        } => format!("{child} -> {parent} ({role})"),
    }
}

pub(super) fn fk_child_columns(fk: &ForeignKey) -> Vec<String> {
    fk.columns.iter().map(|(c, _)| c.clone()).collect()
}

pub(super) fn join_notes(note: Option<String>, more: String) -> String {
    match note {
        Some(n) => format!("{n}; {more}"),
        None => more,
    }
}

pub(super) fn with_note(def: String, column: &ModelColumn) -> String {
    let mut notes = Vec::new();
    if let Some(label) = &column.label {
        notes.push(label.clone());
    }
    if column.is_soft_delete {
        notes.push("soft delete marker".to_string());
    }
    if column.is_history {
        notes.push("history/versioning marker".to_string());
    }
    if column.is_tenant_scope {
        notes.push("tenant scope discriminator".to_string());
    }
    if let Some(expr) = &column.derived_expr {
        notes.push(format!("derived: {}", expr.display()));
    }
    if notes.is_empty() {
        def
    } else {
        format!("{def} /* {} */", notes.join("; ").replace("*/", "* /"))
    }
}

pub(super) fn table_note(entity: &Entity) -> String {
    let mut note = entity.label.clone();
    if let Some(description) = &entity.description {
        if !description.trim().is_empty() {
            note.push_str(": ");
            note.push_str(description.trim());
        }
    }
    note.replace('\n', " ")
}

/// One line: a label or description may hold a line break, which would end a `--` comment.
pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `-- PaymentMethod (支払方法): cash = 現金, card`: a variation's label and its values'.
pub(super) fn variation_note(variation: &rdra_ish_core::model::Variation) -> String {
    let values: Vec<String> = variation
        .values
        .iter()
        .map(|v| match &v.label {
            Some(label) => format!("{} = {}", v.value, one_line(label)),
            None => v.value.clone(),
        })
        .collect();
    format!(
        "-- {} ({}): {}",
        variation.id,
        one_line(&variation.label),
        values.join(", ")
    )
}

pub(super) fn create_enum(name: &str, values: &[String]) -> String {
    format!(
        "CREATE TYPE {} AS ENUM ({});",
        ident(name),
        values
            .iter()
            .map(|v| string(v))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn enum_type_name(entity: &Entity, column: &ModelColumn) -> String {
    format!("{}_{}", entity.id, column.name)
}

pub(super) fn ref_action(action: RefAction) -> String {
    action.as_str().replace('_', " ").to_ascii_uppercase()
}

/// The condition a row not soft-deleted meets: the marker is null, or `FALSE` for a `Bool`.
pub(super) fn live_condition(marker: &ModelColumn) -> Option<String> {
    let col = ident(&marker.name);
    match marker.col_type {
        ColumnType::Bool => Some(format!("{col} = FALSE")),
        _ if marker.is_nullable => Some(format!("{col} IS NULL")),
        _ => None,
    }
}

pub(super) fn same_set(a: &[String], b: &[String]) -> bool {
    let a: BTreeSet<&String> = a.iter().collect();
    let b: BTreeSet<&String> = b.iter().collect();
    a == b
}

pub(super) fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(super) fn idents(names: &[String]) -> String {
    names
        .iter()
        .map(|n| ident(n))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
