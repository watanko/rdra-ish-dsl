//! Runtime functions, validation, and row conversion for generated Python domains.

use super::*;
use crate::codegen::ir::CmpOp;

// ── Use cases ────────────────────────────────────────────────────────────────

pub(super) fn print_functions(
    domain: &DomainModel,
    enums: &EnumMembers,
    need_unreachable: bool,
    need_unwrap: bool,
    out: &mut String,
) {
    if !domain.events.is_empty() {
        out.push_str("# ── Events ──\n\n");
    }
    for event in &domain.events {
        out.push_str("@_dc.dataclass(frozen=True)\n");
        out.push_str(&format!("class {}:\n", event.name));
        let doc = docstring(&event.doc, "    ");
        if !doc.is_empty() {
            out.push_str(&doc);
        }
        out.push_str(&format!(
            "    event: _t.Literal[\"{0}\"] = _dc.field(default=\"{0}\", init=False)\n",
            event.name
        ));
        for field in &event.fields {
            out.push_str(&comment(
                &format!("The {} row the event was raised on.", field.entity),
                "    ",
            ));
            out.push_str(&format!("    {}: {}\n", ident(&field.name), field.id_ty));
        }
        out.push_str("\n\n");
    }

    if domain.fns.is_empty() {
        return;
    }

    if !domain.marks.is_empty() {
        out.push_str("# ── Run marks of use cases split into transactions ──\n\n");
    }
    for m in &domain.marks {
        out.push_str("@_dc.dataclass(frozen=True)\n");
        out.push_str(&format!("class {}:\n", m.name));
        out.push_str(&docstring(
            &[format!(
                "The mark step {} of {} leaves on a {} row: the next step takes it. Store it with the row.",
                m.step, m.usecase, m.entity
            )],
            "    ",
        ));
        out.push_str(&format!("    id: {}\n", m.id_ty));
        out.push_str(&format!(
            "    usecase: _t.Literal[{0}] = _dc.field(default={0}, init=False)\n",
            string_literal(&m.usecase)
        ));
        out.push_str(&format!(
            "    step: _t.Literal[{0}] = _dc.field(default={0}, init=False)\n",
            m.step
        ));
        out.push_str("\n\n");
    }

    out.push_str("# ── Use cases ──\n\n");
    out.push_str("@_dc.dataclass(frozen=True)\n");
    out.push_str("class Rejected:\n");
    out.push_str(
        "    \"\"\"A condition of the use case did not hold: the use case does not run.\"\"\"\n",
    );
    out.push_str("    condition: str\n");
    out.push_str("    label: str\n");
    out.push_str("    site: str | None\n\n\n");

    if need_unreachable {
        out.push_str("def _unreachable(message: str) -> _t.NoReturn:\n");
        out.push_str("    \"\"\"A state the verified model does not reach: the arguments break a rule.\"\"\"\n");
        out.push_str("    raise RuntimeError(message)\n\n\n");
    }
    if need_unwrap {
        out.push_str("_T = _t.TypeVar(\"_T\")\n\n\n");
        out.push_str("def _unwrap(value: _T | None, message: str) -> _T:\n");
        out.push_str("    if value is None:\n");
        out.push_str("        raise RuntimeError(message)\n");
        out.push_str("    return value\n\n\n");
    }

    for f in &domain.fns {
        let fname = fn_ident(&f.name);
        // Result dataclass.
        out.push_str("@_dc.dataclass(frozen=True)\n");
        out.push_str(&format!("class {}:\n", f.result));
        out.push_str(&docstring(&[format!("What `{fname}` returns.")], "    "));
        for o in &f.outputs {
            match &o.kind {
                OutKind::Row { ty, .. } => {
                    out.push_str(&format!("    {}: {ty}\n", ident(&o.name)));
                }
                OutKind::Deleted {
                    id: Some((ty, _)), ..
                } => {
                    out.push_str(&format!("    {}: {ty}\n", ident(&o.name)));
                }
                OutKind::Deleted { id: None, .. } => {}
            }
        }
        for m in &f.marks_out {
            out.push_str(&format!("    {}: {}\n", ident(&m.name), m.ty));
        }
        for event in &f.raises {
            out.push_str(&format!("    {}: {}\n", ident(&snake(&event.ty)), event.ty));
        }
        out.push_str("\n\n");

        // Function signature and body.
        let mut params: Vec<String> = f
            .rows
            .iter()
            .map(|r| format!("{}: {}", ident(&r.name), r.ty))
            .collect();
        for m in &f.marks_in {
            params.push(format!("{}: {}", ident(&m.name), m.ty));
        }
        if let Some(ev) = &f.event_in {
            params.push(format!("{}: {}", ident(&ev.name), ev.ty));
        }
        for i in &f.inputs {
            let ty = field_type(&i.ty);
            let ty = if i.nullable {
                format!("{ty} | None")
            } else {
                ty
            };
            params.push(format!("{}: {ty}", ident(&i.name)));
        }
        let ret = if f.conditions.is_empty() {
            f.result.clone()
        } else {
            format!("{} | Rejected", f.result)
        };
        out.push_str(&format!("def {fname}({}) -> {ret}:\n", params.join(", ")));

        let mut lines = f.doc.clone();
        for c in &f.conditions {
            lines.push(format!(
                "Requires the condition {} ({}){}.",
                c.id,
                one_line(&c.label),
                c.site
                    .as_ref()
                    .map(|s| format!(" at {s}"))
                    .unwrap_or_default()
            ));
        }
        let doc = docstring(&lines, "    ");
        if !doc.is_empty() {
            out.push_str(&doc);
        }

        let row = |r: usize| match f.rows.get(r) {
            Some(x) => ident(&x.name),
            None => ident(&f.elems[r - f.rows.len()].name),
        };
        let input_var = |i: usize| ident(&f.inputs[i].name);

        for check in &f.checks {
            match check {
                ArgCheck::Link {
                    child,
                    child_field,
                    parent,
                    parent_field,
                    message,
                } => {
                    out.push_str(&format!(
                        "    if {}.{} != {}.{}:\n        raise ValueError({})\n",
                        row(*child),
                        ident(&child_field.wire),
                        row(*parent),
                        ident(&parent_field.wire),
                        string_literal(message)
                    ));
                }
                ArgCheck::EventRow {
                    row: r,
                    field,
                    event_field,
                    message,
                } => {
                    let ev = f
                        .event_in
                        .as_ref()
                        .expect("an event row check has an event");
                    out.push_str(&format!(
                        "    if {}.{} != {}.{}:\n        raise ValueError({})\n",
                        row(*r),
                        ident(&field.wire),
                        ident(&ev.name),
                        ident(event_field),
                        string_literal(message)
                    ));
                }
                ArgCheck::MustBeNull {
                    row: r,
                    field,
                    message,
                } => {
                    out.push_str(&format!(
                        "    if {}.{} is not None:\n        _unreachable({})\n",
                        row(*r),
                        ident(&field.wire),
                        string_literal(message)
                    ));
                }
            }
        }

        for c in &f.conditions {
            out.push_str(&format!(
                "    if {}:\n        return Rejected(condition={}, label={}, site={})\n",
                expr(&fails(&c.expr), enums, &row, &input_var),
                string_literal(&c.id),
                string_literal(&c.label),
                c.site
                    .as_ref()
                    .map_or("None".to_string(), |s| string_literal(s))
            ));
        }

        let mut result_args = Vec::new();
        for o in &f.outputs {
            match &o.kind {
                OutKind::Row { ty, fields, .. } => {
                    if fields.is_empty() {
                        result_args.push(format!("        {}={ty}(),\n", ident(&o.name)));
                    } else {
                        let mut block = format!("        {}={ty}(\n", ident(&o.name));
                        for (target, assigned) in fields {
                            let v = match assigned {
                                Assigned::From {
                                    row: r,
                                    field,
                                    adapt,
                                } => {
                                    let read = format!("{}.{}", row(*r), ident(&field.wire));
                                    match adapt {
                                        Adapt::Same | Adapt::Wrap => read,
                                        Adapt::Unwrap(message) => {
                                            format!("_unwrap({read}, {})", string_literal(message))
                                        }
                                        Adapt::AbsentNull => "None".to_string(),
                                    }
                                }
                                Assigned::Lit { lit, .. } => lit_py(lit, enums),
                                Assigned::Input { input: i, .. } => input_var(*i),
                                Assigned::ParentId { row: r, field, .. } => {
                                    format!("{}.{}", row(*r), ident(&field.wire))
                                }
                                Assigned::Switch {
                                    row: r,
                                    field,
                                    enum_ty,
                                    arms,
                                } => {
                                    let pairs: Vec<String> = arms
                                        .iter()
                                        .map(|(from, to)| {
                                            format!(
                                                "{enum_ty}.{}: {enum_ty}.{}",
                                                enums.member(enum_ty, from),
                                                enums.member(enum_ty, to)
                                            )
                                        })
                                        .collect();
                                    format!(
                                        "_unwrap({{{}}}.get({}.{}), {})",
                                        pairs.join(", "),
                                        row(*r),
                                        ident(&field.wire),
                                        string_literal(&format!(
                                            "rdra-ish: {} is in no state {} moves it from",
                                            field.wire, f.usecase
                                        ))
                                    )
                                }
                                Assigned::EventId { field, .. } => format!(
                                    "{}.{}",
                                    ident(
                                        &f.event_in
                                            .as_ref()
                                            .expect("an event id has an event")
                                            .name
                                    ),
                                    ident(field)
                                ),
                                Assigned::MarkId { mark, .. } => {
                                    format!("{}.id", ident(&f.marks_in[*mark].name))
                                }
                                Assigned::Null => "None".to_string(),
                            };
                            block.push_str(&format!("            {}={v},\n", ident(&target.wire)));
                        }
                        block.push_str("        ),\n");
                        result_args.push(block);
                    }
                }
                OutKind::Deleted {
                    row: r,
                    id: Some((_, field)),
                } => {
                    result_args.push(format!(
                        "        {}={}.{},\n",
                        ident(&o.name),
                        row(*r),
                        ident(&field.wire)
                    ));
                }
                OutKind::Deleted { id: None, .. } => {}
            }
        }
        for m in &f.marks_out {
            let id = mark_source_py(&m.source, &row, &input_var, &f.marks_in);
            result_args.push(format!("        {}={}(id={id}),\n", ident(&m.name), m.ty));
        }
        for event in &f.raises {
            let mut parts = Vec::new();
            for (field, source) in &event.ids {
                let id = mark_source_py(source, &row, &input_var, &f.marks_in);
                parts.push(format!("{}={id}", ident(&field.name)));
            }
            result_args.push(format!(
                "        {}={}({}),\n",
                ident(&snake(&event.ty)),
                event.ty,
                parts.join(", ")
            ));
        }

        if result_args.is_empty() {
            out.push_str(&format!("    return {}()\n\n\n", f.result));
        } else {
            out.push_str(&format!("    return {}(\n", f.result));
            for arg in result_args {
                out.push_str(&arg);
            }
            out.push_str("    )\n\n\n");
        }
    }
}

fn mark_source_py(
    source: &MarkSource,
    row: &dyn Fn(usize) -> String,
    input_var: &dyn Fn(usize) -> String,
    marks_in: &[super::super::ir::MarkIn],
) -> String {
    match source {
        MarkSource::Row { row: r, field } => format!("{}.{}", row(*r), ident(&field.wire)),
        MarkSource::Input(i) => input_var(*i),
        MarkSource::Mark(k) => format!("{}.id", ident(&marks_in[*k].name)),
    }
}

fn lit_py(lit: &Lit, enums: &EnumMembers) -> String {
    match lit {
        Lit::Enum { ty, value } => enums.expression(ty, value),
        Lit::Bool(true) => "True".to_string(),
        Lit::Bool(false) => "False".to_string(),
        Lit::Int(n) => n.to_string(),
    }
}

fn expr(
    e: &Expr,
    enums: &EnumMembers,
    row: &dyn Fn(usize) -> String,
    input: &dyn Fn(usize) -> String,
) -> String {
    match e {
        Expr::True => "True".to_string(),
        Expr::False => "False".to_string(),
        Expr::Not(inner) => match inner.as_ref() {
            Expr::IsNull { row: r, field } => {
                format!("{}.{} is not None", row(*r), ident(&field.wire))
            }
            Expr::BoolEq {
                row: r,
                field,
                value,
            } => {
                let b = if *value { "True" } else { "False" };
                format!("{}.{} is not {b}", row(*r), ident(&field.wire))
            }
            Expr::EnumEq {
                row: r,
                field,
                enum_ty,
                value,
            } => format!(
                "{}.{} != {}",
                row(*r),
                ident(&field.wire),
                enums.expression(enum_ty, value)
            ),
            Expr::KeyEq {
                child,
                child_field,
                parent,
                parent_field,
            } => format!(
                "{}.{} != {}.{}",
                row(*child),
                ident(&child_field.wire),
                row(*parent),
                ident(&parent_field.wire)
            ),
            _ => format!("not ({})", expr(inner, enums, row, input)),
        },
        Expr::And(items) if items.is_empty() => "True".to_string(),
        Expr::Or(items) if items.is_empty() => "False".to_string(),
        Expr::And(items) => items
            .iter()
            .map(|i| format!("({})", expr(i, enums, row, input)))
            .collect::<Vec<_>>()
            .join(" and "),
        Expr::Or(items) => items
            .iter()
            .map(|i| format!("({})", expr(i, enums, row, input)))
            .collect::<Vec<_>>()
            .join(" or "),
        Expr::IsNull { row: r, field } => format!("{}.{} is None", row(*r), ident(&field.wire)),
        Expr::EnumEq {
            row: r,
            field,
            enum_ty,
            value,
        } => format!(
            "{}.{} == {}",
            row(*r),
            ident(&field.wire),
            enums.expression(enum_ty, value)
        ),
        Expr::BoolEq {
            row: r,
            field,
            value,
        } => {
            let b = if *value { "True" } else { "False" };
            format!("{}.{} is {b}", row(*r), ident(&field.wire))
        }
        Expr::IntCmp {
            row: r,
            field,
            op,
            value,
        } => {
            let value_at = format!("{}.{}", row(*r), ident(&field.wire));
            let comparison = format!("{value_at} {} {value}", op.symbol());
            if field.nullable && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                format!("({value_at} is not None and {comparison})")
            } else {
                comparison
            }
        }
        Expr::FieldCmp {
            left,
            left_field,
            op,
            right,
            right_field,
        } => {
            let left_at = format!("{}.{}", row(*left), ident(&left_field.wire));
            let right_at = format!("{}.{}", row(*right), ident(&right_field.wire));
            let mut parts = Vec::new();
            if left_field.nullable && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                parts.push(format!("{left_at} is not None"));
            }
            if right_field.nullable && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                parts.push(format!("{right_at} is not None"));
            }
            parts.push(format!("{left_at} {} {right_at}", op.symbol()));
            format!("({})", parts.join(" and "))
        }
        Expr::KeyEq {
            child,
            child_field,
            parent,
            parent_field,
        } => format!(
            "{}.{} == {}.{}",
            row(*child),
            ident(&child_field.wire),
            row(*parent),
            ident(&parent_field.wire)
        ),
        Expr::Exists {
            input: i,
            elem,
            body,
        } => format!(
            "any({} for {} in {})",
            expr(body, enums, row, input),
            row(*elem),
            input(*i)
        ),
        Expr::Forall {
            input: i,
            elem,
            body,
        } => format!(
            "all({} for {} in {})",
            expr(body, enums, row, input),
            row(*elem),
            input(*i)
        ),
    }
}

// ── Rule checks ──────────────────────────────────────────────────────────────

pub(super) fn print_validators(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
    if domain.validators.is_empty() {
        return;
    }
    out.push_str("# ── Rule checks ──\n\n");
    let row = |_: usize| "row".to_string();
    for v in &domain.validators {
        out.push_str(&format!(
            "def {}_rule_violations(row: {}) -> list[str]:\n",
            fn_ident(&v.ty),
            v.ty
        ));
        out.push_str(&docstring(
            &[format!(
                "The rules a {} row breaks, by the names `check` reports: empty when it keeps them all.",
                v.ty
            )],
            "    ",
        ));
        if v.rules.is_empty() {
            out.push_str("    return []\n\n\n");
            continue;
        }
        out.push_str("    out: list[str] = []\n");
        for rule in &v.rules {
            out.push_str(&format!(
                "    if {}:  # {}\n        out.append({})\n",
                expr(&fails(&rule.expr), enums, &row, &|_| String::new()),
                one_line(&rule.text),
                string_literal(&rule.name)
            ));
        }
        out.push_str("    return out\n\n\n");
    }
    for sum in &domain.sums {
        out.push_str(&format!(
            "def {}_rule_violations(row: {}) -> list[str]:\n",
            fn_ident(&sum.name),
            sum.name
        ));
        out.push_str(&docstring(
            &[format!(
                "The rules a {} row breaks, by its state: empty when it keeps them all.",
                sum.name
            )],
            "    ",
        ));
        let last = sum.variants.len().saturating_sub(1);
        for (idx, variant) in sum.variants.iter().enumerate() {
            let call = format!("{}_rule_violations(row)", fn_ident(&variant.name));
            if idx < last {
                out.push_str(&format!(
                    "    if isinstance(row, {}):\n        return {call}\n",
                    variant.name
                ));
            } else {
                out.push_str(&format!("    return {call}\n"));
            }
        }
        out.push_str("\n\n");
    }
}

/// The condition under which `e` fails, folded so that a negated comparison reads plainly.
fn fails(e: &Expr) -> Expr {
    super::super::actions::simplify(Expr::Not(Box::new(e.clone())))
}

// ── Rows and dispatch ────────────────────────────────────────────────────────

pub(super) fn print_rows(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
    if domain.sums.is_empty() {
        return;
    }
    out.push_str("# ── Rows: the flat shape a store or the wire holds ──\n\n");
    out.push_str("@_dc.dataclass(frozen=True)\n");
    out.push_str("class InvalidRow:\n");
    out.push_str("    \"\"\"A row that breaks what the type of its state requires.\"\"\"\n");
    out.push_str("    entity: str\n");
    out.push_str("    column: str\n");
    out.push_str("    reason: str\n\n\n");

    for sum in &domain.sums {
        let FieldType::Enum(tag_enum) = &sum.tag.ty else {
            continue;
        };
        let shadows = field_shadows(&sum.row_fields, None);
        out.push_str("@_dc.dataclass(frozen=True)\n");
        out.push_str(&format!("class {}:\n", sum.row));
        out.push_str(&docstring(
            &[format!(
                "Every column of {} as the data layer declares it.",
                sum.entity
            )],
            "    ",
        ));
        for f in &sum.row_fields {
            let ty = field_type_in_class(&f.ty, &shadows);
            let ty = if f.nullable {
                format!("{ty} | None")
            } else {
                ty
            };
            out.push_str(&format!("    {}: {ty}\n", ident(&f.wire)));
        }
        out.push_str("\n\n");

        // from_row
        out.push_str(&format!(
            "def {}_from_row(row: {}) -> {} | InvalidRow:\n",
            fn_ident(&sum.name),
            sum.row,
            sum.name
        ));
        out.push_str(&docstring(
            &[format!(
                "The state of a {} row, or why the row fits none.",
                sum.entity
            )],
            "    ",
        ));
        let tag_col = ident(&sum.tag.wire);
        for v in &sum.variants {
            let member = enums.member(tag_enum, &v.value);
            out.push_str(&format!("    if row.{tag_col} == {tag_enum}.{member}:\n"));
            for f in &v.always_null {
                let col = ident(&f.wire);
                out.push_str(&format!(
                    "        if row.{col} is not None:\n            return InvalidRow(entity={}, column={}, reason={})\n",
                    string_literal(&sum.entity),
                    string_literal(&f.wire),
                    string_literal(&format!("null in the state {}", v.value.wire))
                ));
            }
            for f in &v.fields {
                let declared = sum.row_fields.iter().find(|r| r.wire == f.wire);
                if declared.is_some_and(|d| d.nullable) && !f.nullable {
                    let col = ident(&f.wire);
                    out.push_str(&format!(
                        "        if row.{col} is None:\n            return InvalidRow(entity={}, column={}, reason={})\n",
                        string_literal(&sum.entity),
                        string_literal(&f.wire),
                        string_literal(&format!("present in the state {}", v.value.wire))
                    ));
                }
            }
            if v.fields.is_empty() {
                out.push_str(&format!("        return {}()\n", v.name));
            } else {
                out.push_str(&format!("        return {}(\n", v.name));
                for f in &v.fields {
                    let col = ident(&f.wire);
                    out.push_str(&format!("            {col}=row.{col},\n"));
                }
                out.push_str("        )\n");
            }
        }
        out.push_str(&format!(
            "    return InvalidRow(entity={}, column={}, reason=f\"unknown state {{row.{tag_col}!r}}\")\n\n\n",
            string_literal(&sum.entity),
            string_literal(&sum.tag.wire)
        ));

        // to_row
        out.push_str(&format!(
            "def {}_to_row(state: {}) -> {}:\n",
            fn_ident(&sum.name),
            sum.name,
            sum.row
        ));
        out.push_str(&docstring(
            &[format!("The flat row of a {}.", sum.entity)],
            "    ",
        ));
        let last = sum.variants.len().saturating_sub(1);
        for (idx, v) in sum.variants.iter().enumerate() {
            let member = enums.member(tag_enum, &v.value);
            let body_indent = if idx < last {
                out.push_str(&format!("    if isinstance(state, {}):\n", v.name));
                "        "
            } else {
                "    "
            };
            out.push_str(&format!("{body_indent}return {}(\n", sum.row));
            for f in &sum.row_fields {
                let col = ident(&f.wire);
                if f.wire == sum.tag.wire {
                    out.push_str(&format!("{body_indent}    {col}={tag_enum}.{member},\n"));
                } else if v.always_null.iter().any(|n| n.wire == f.wire) {
                    out.push_str(&format!("{body_indent}    {col}=None,\n"));
                } else {
                    out.push_str(&format!("{body_indent}    {col}=state.{col},\n"));
                }
            }
            out.push_str(&format!("{body_indent})\n"));
        }
        out.push_str("\n\n");
    }
}

pub(super) fn print_dispatchers(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
    if domain.dispatchers.is_empty() {
        return;
    }
    out.push_str("# ── Dispatch: run a use case from whatever states its rows are in ──\n\n");
    out.push_str("@_dc.dataclass(frozen=True)\n");
    out.push_str("class NotEnabled:\n");
    out.push_str("    \"\"\"The rows are in states the use case does not run from.\"\"\"\n");
    out.push_str("    usecase: str\n\n\n");

    for d in &domain.dispatchers {
        let dname = fn_ident(&d.name);
        out.push_str(&comment(
            &format!("What `{dname}` returns when the use case runs."),
            "",
        ));
        let mut results: Vec<&str> = Vec::new();
        for c in &d.cases {
            if !results.contains(&c.result.as_str()) {
                results.push(&c.result);
            }
        }
        out.push_str(&format!(
            "{}: _t.TypeAlias = {}\n\n\n",
            d.outcome,
            results.join(" | ")
        ));

        // Avoid shadowing any transition function called by this dispatcher.
        let called_fns: BTreeSet<String> = d.cases.iter().map(|c| fn_ident(&c.function)).collect();
        let param_name = |raw: &str| {
            let mut n = ident(raw);
            while called_fns.contains(&n) {
                n.push_str("_arg");
            }
            n
        };

        let mut params: Vec<String> = d
            .rows
            .iter()
            .map(|(n, ty)| format!("{}: {ty}", param_name(n)))
            .collect();
        for (n, ty) in &d.marks {
            params.push(format!("{}: {ty}", param_name(n)));
        }
        for i in &d.inputs {
            let ty = field_type(&i.ty);
            let ty = if i.nullable {
                format!("{ty} | None")
            } else {
                ty
            };
            params.push(format!("{}: {ty}", param_name(&i.name)));
        }
        for (n, ty) in &d.choices {
            params.push(format!("{}: {ty}", param_name(n)));
        }
        let ret = if d.rejects {
            format!("{} | Rejected | NotEnabled", d.outcome)
        } else {
            format!("{} | NotEnabled", d.outcome)
        };
        out.push_str(&format!("def {dname}({}) -> {ret}:\n", params.join(", ")));
        out.push_str(&docstring(
            &[format!(
                "Runs {} from whatever states its rows are in: the transition function of those states, or NotEnabled.",
                d.usecase
            )],
            "    ",
        ));

        let mut unconditional = false;
        for c in &d.cases {
            let tests: Vec<String> = c
                .states
                .iter()
                .zip(&d.rows)
                .filter_map(|(s, (n, _))| {
                    let (variant_ty, _, _) = s.as_ref()?;
                    Some(format!("isinstance({}, {variant_ty})", param_name(n)))
                })
                .chain(c.choice.iter().map(|(i, v)| {
                    let (n, ty) = &d.choices[*i];
                    format!("{} == {}", param_name(n), enums.expression(ty, v))
                }))
                .collect();
            let mut args: Vec<String> = d.rows.iter().map(|(n, _)| param_name(n)).collect();
            args.extend(d.marks.iter().map(|(n, _)| param_name(n)));
            args.extend(c.inputs.iter().map(|i| param_name(&d.inputs[*i].name)));
            let call = format!("{}({})", fn_ident(&c.function), args.join(", "));
            if tests.is_empty() {
                out.push_str(&format!("    return {call}\n"));
                unconditional = true;
                break;
            } else {
                out.push_str(&format!(
                    "    if {}:\n        return {call}\n",
                    tests.join(" and ")
                ));
            }
        }
        if !unconditional {
            out.push_str(&format!(
                "    return NotEnabled(usecase={})\n",
                string_literal(&d.usecase)
            ));
        }
        out.push_str("\n\n");
    }
}
