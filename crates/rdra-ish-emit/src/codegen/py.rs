//! The Python printer: `NewType` scalars and ids, `(str, Enum)` value sets, frozen
//! dataclasses, union state types, transition functions, rule checks, row conversions, and
//! dispatchers. Targets Python 3.10+ with standard library imports only.

use std::collections::{BTreeMap, BTreeSet};

use super::actions::snake;
use super::ir::{
    Adapt, ArgCheck, Assigned, DomainModel, EnumValue, Expr, Field, FieldType, Lit, MarkSource,
    OutKind, RecordKind, Scalar,
};
use super::{header_lines, obligation_lines, one_line, string_literal, Options};

pub(super) fn print(domain: &DomainModel, options: &Options) -> String {
    let mut out = String::new();
    for line in header_lines(options) {
        out.push_str(&comment(&line, ""));
    }
    out.push_str("#\n");
    for line in obligation_lines(domain) {
        out.push_str(&comment(&line, ""));
    }
    out.push('\n');

    let enums = EnumMembers::build(domain);
    let need_builtins = any_builtin_shadow(domain);
    let need_abc = any_rows_field(domain);
    let need_dc = !domain.records.is_empty()
        || !domain.sums.is_empty()
        || !domain.events.is_empty()
        || !domain.marks.is_empty()
        || !domain.fns.is_empty()
        || !domain.dispatchers.is_empty();
    let need_enum = !domain.enums.is_empty();
    let need_unreachable = domain.fns.iter().any(|f| {
        f.checks
            .iter()
            .any(|c| matches!(c, ArgCheck::MustBeNull { .. }))
    });
    let need_unwrap = domain.fns.iter().any(|f| {
        f.outputs.iter().any(|o| match &o.kind {
            OutKind::Row { fields, .. } => fields.iter().any(|(_, a)| {
                matches!(
                    a,
                    Assigned::From {
                        adapt: Adapt::Unwrap(_),
                        ..
                    } | Assigned::Switch { .. }
                )
            }),
            OutKind::Deleted { .. } => false,
        })
    });

    out.push_str("from __future__ import annotations\n\n");
    if need_builtins {
        out.push_str("import builtins as _builtins\n");
    }
    if need_abc {
        out.push_str("import collections.abc as _abc\n");
    }
    if need_dc {
        out.push_str("import dataclasses as _dc\n");
    }
    if need_enum {
        out.push_str("import enum as _enum\n");
    }
    out.push_str("import typing as _t\n\n");

    out.push_str("# ── Scalars: replace with the project's own types if it has them ──\n\n");
    for s in Scalar::PRELUDE {
        out.push_str(&comment(s.default_doc(), ""));
        out.push_str(&format!("{0} = _t.NewType(\"{0}\", str)\n\n", s.name()));
    }

    if !domain.ids.is_empty() {
        out.push_str("# ── Ids ──\n\n");
    }
    for id in &domain.ids {
        out.push_str(&comment(&format!("The id of entity {}.", id.entity), ""));
        out.push_str(&format!(
            "{0} = _t.NewType(\"{0}\", {1})\n\n",
            id.name,
            scalar(id.repr, false)
        ));
    }

    if !domain.enums.is_empty() {
        out.push_str("# ── Value sets ──\n\n");
    }
    for decl in &domain.enums {
        out.push_str(&format!("class {}(str, _enum.Enum):\n", decl.name));
        let doc = docstring(&decl.doc, "    ");
        if !doc.is_empty() {
            out.push_str(&doc);
        }
        if decl.values.is_empty() {
            if doc.is_empty() {
                out.push_str("    pass\n");
            }
        } else {
            for value in &decl.values {
                let member = enums.member(&decl.name, value);
                let label = value
                    .label
                    .as_ref()
                    .map(|l| format!("  # {}", one_line(l)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    {member} = {}{label}\n",
                    string_literal(&value.wire)
                ));
            }
        }
        out.push('\n');
        out.push_str(&format!(
            "{}_VALUES: tuple[{}, ...] = tuple({})\n\n\n",
            snake(&decl.name).to_ascii_uppercase(),
            decl.name,
            decl.name
        ));
    }

    for kind in [RecordKind::Entity, RecordKind::Dto] {
        let records: Vec<_> = domain.records.iter().filter(|r| r.kind == kind).collect();
        if records.is_empty() {
            continue;
        }
        out.push_str(match kind {
            RecordKind::Entity => "# ── Entities ──\n\n",
            RecordKind::Dto => "# ── DTOs ──\n\n",
        });
        for record in records {
            let shadows = field_shadows(&record.fields, None);
            out.push_str("@_dc.dataclass(frozen=True)\n");
            out.push_str(&format!("class {}:\n", record.name));
            let doc = docstring(&record.doc, "    ");
            if !doc.is_empty() {
                out.push_str(&doc);
            }
            if record.fields.is_empty() {
                if doc.is_empty() {
                    out.push_str("    pass\n");
                }
            } else {
                for field in &record.fields {
                    for line in &field.doc {
                        out.push_str(&comment(&one_line(line), "    "));
                    }
                    let ty = field_type_in_class(&field.ty, &shadows);
                    let ty = if field.nullable {
                        format!("{ty} | None")
                    } else {
                        ty
                    };
                    out.push_str(&format!("    {}: {ty}\n", ident(&field.wire)));
                }
            }
            out.push_str("\n\n");
        }
    }

    if !domain.sums.is_empty() {
        out.push_str("# ── Entities with states ──\n\n");
    }
    for sum in &domain.sums {
        let FieldType::Enum(tag_enum) = &sum.tag.ty else {
            unreachable!("a tag column is an enum");
        };
        for variant in &sum.variants {
            let mut lines = variant.doc.clone();
            for field in &variant.always_null {
                lines.push(format!(
                    "`{}`: {}",
                    field.wire,
                    field.doc.last().cloned().unwrap_or_default()
                ));
            }
            let shadows = field_shadows(&variant.fields, Some(&sum.tag.wire));
            out.push_str("@_dc.dataclass(frozen=True)\n");
            out.push_str(&format!("class {}:\n", variant.name));
            let doc = docstring(&lines, "    ");
            if !doc.is_empty() {
                out.push_str(&doc);
            }
            let tag_member = enums.member(tag_enum, &variant.value);
            out.push_str(&format!(
                "    {}: _t.Literal[{tag_enum}.{tag_member}] = _dc.field(default={tag_enum}.{tag_member}, init=False)\n",
                ident(&sum.tag.wire)
            ));
            for field in &variant.fields {
                for line in &field.doc {
                    out.push_str(&comment(&one_line(line), "    "));
                }
                let ty = field_type_in_class(&field.ty, &shadows);
                let ty = if field.nullable {
                    format!("{ty} | None")
                } else {
                    ty
                };
                out.push_str(&format!("    {}: {ty}\n", ident(&field.wire)));
            }
            out.push_str("\n\n");
        }
        let mut lines = sum.doc.clone();
        lines.push(format!(
            "One of its states, told apart by `{}`.",
            sum.tag.wire
        ));
        for line in &lines {
            out.push_str(&comment(&one_line(line), ""));
        }
        let variants: Vec<&str> = sum.variants.iter().map(|v| v.name.as_str()).collect();
        out.push_str(&format!(
            "{}: _t.TypeAlias = {}\n\n\n",
            sum.name,
            variants.join(" | ")
        ));
    }

    print_functions(domain, &enums, need_unreachable, need_unwrap, &mut out);
    print_validators(domain, &enums, &mut out);
    print_rows(domain, &enums, &mut out);
    print_dispatchers(domain, &enums, &mut out);

    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Deterministic, collision-free `SCREAMING_SNAKE_CASE` enum member names per enum.
struct EnumMembers {
    members: BTreeMap<(String, String), String>,
}

impl EnumMembers {
    fn build(domain: &DomainModel) -> Self {
        let mut members = BTreeMap::new();
        for decl in &domain.enums {
            let mut seen = BTreeSet::new();
            for value in &decl.values {
                let mut base = snake(&value.ident).to_ascii_uppercase();
                if base.is_empty() || base.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    base = format!("V_{base}");
                }
                if is_keyword(&base) || base == "MRO" {
                    base.push('_');
                }
                let mut candidate = base.clone();
                let mut n = 2;
                while !seen.insert(candidate.clone()) {
                    candidate = format!("{base}_{n}");
                    n += 1;
                }
                members.insert((decl.name.clone(), value.wire.clone()), candidate);
            }
        }
        Self { members }
    }

    fn member(&self, enum_name: &str, value: &EnumValue) -> String {
        self.members
            .get(&(enum_name.to_string(), value.wire.clone()))
            .cloned()
            .unwrap_or_else(|| snake(&value.ident).to_ascii_uppercase())
    }
}

fn any_builtin_shadow(domain: &DomainModel) -> bool {
    let has_shadow = |fields: &[Field], extra: Option<&str>| {
        !field_shadows(fields, extra).is_empty()
    };
    domain
        .records
        .iter()
        .any(|r| has_shadow(&r.fields, None))
        || domain.sums.iter().any(|s| {
            has_shadow(&s.row_fields, None)
                || s.variants
                    .iter()
                    .any(|v| has_shadow(&v.fields, Some(&s.tag.wire)))
        })
}

fn field_shadows(fields: &[Field], extra: Option<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in fields
        .iter()
        .map(|f| ident(&f.wire))
        .chain(extra.map(ident))
    {
        if matches!(name.as_str(), "int" | "str" | "bool") {
            out.insert(name);
        }
    }
    out
}

fn any_rows_field(domain: &DomainModel) -> bool {
    let is_rows = |ty: &FieldType| matches!(ty, FieldType::Rows(_));
    domain
        .records
        .iter()
        .any(|r| r.fields.iter().any(|f| is_rows(&f.ty)))
        || domain
            .fns
            .iter()
            .any(|f| f.inputs.iter().any(|i| is_rows(&i.ty)))
        || domain
            .dispatchers
            .iter()
            .any(|d| d.inputs.iter().any(|i| is_rows(&i.ty)))
}

fn field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Scalar(s) => scalar(*s, false).to_string(),
        FieldType::Enum(name) | FieldType::Id(name) => name.clone(),
        FieldType::Rows(name) => format!("_abc.Sequence[{name}]"),
    }
}

fn field_type_in_class(ty: &FieldType, shadows: &BTreeSet<String>) -> String {
    match ty {
        FieldType::Scalar(s) => {
            let plain = scalar(*s, false);
            let shadowed = shadows.contains(plain);
            scalar(*s, shadowed).to_string()
        }
        FieldType::Enum(name) | FieldType::Id(name) => name.clone(),
        FieldType::Rows(name) => format!("_abc.Sequence[{name}]"),
    }
}

fn scalar(s: Scalar, qualified: bool) -> &'static str {
    match (s, qualified) {
        (Scalar::Int, false) => "int",
        (Scalar::Int, true) => "_builtins.int",
        (Scalar::String, false) => "str",
        (Scalar::String, true) => "_builtins.str",
        (Scalar::Bool, false) => "bool",
        (Scalar::Bool, true) => "_builtins.bool",
        (other, _) => other.name(),
    }
}

fn is_keyword(name: &str) -> bool {
    const KEYWORDS: [&str; 35] = [
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ];
    KEYWORDS.contains(&name)
}

/// A Python identifier: non-identifier punctuation becomes `_`, leading digits get `_`, and
/// Python keywords plus `self` get a trailing `_`.
fn ident(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        clean.insert(0, '_');
    }
    if is_keyword(&clean) || clean == "self" {
        clean.push('_');
    }
    clean
}

fn fn_ident(pascal: &str) -> String {
    ident(&snake(pascal))
}

fn comment(line: &str, indent: &str) -> String {
    if line.is_empty() {
        format!("{indent}#\n")
    } else {
        format!("{indent}# {line}\n")
    }
}

fn escape_docstring(text: &str) -> String {
    let mut s = one_line(text).replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
    if s.ends_with('"') {
        s.push(' ');
    }
    s
}

fn docstring(lines: &[String], indent: &str) -> String {
    let lines: Vec<String> = lines
        .iter()
        .map(|l| escape_docstring(l))
        .filter(|l| !l.is_empty())
        .collect();
    match lines.as_slice() {
        [] => String::new(),
        [one] => format!("{indent}\"\"\"{one}\"\"\"\n"),
        [first, rest @ ..] => {
            let mut out = format!("{indent}\"\"\"{first}\n\n");
            for line in rest {
                out.push_str(&format!("{indent}{line}\n"));
            }
            out.push_str(&format!("{indent}\"\"\"\n"));
            out
        }
    }
}

// ── Use cases ────────────────────────────────────────────────────────────────

fn print_functions(
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
    out.push_str("    \"\"\"A condition of the use case did not hold: the use case does not run.\"\"\"\n");
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
            out.push_str(&format!(
                "    {}: {}\n",
                ident(&snake(&event.ty)),
                event.ty
            ));
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
    marks_in: &[super::ir::MarkIn],
) -> String {
    match source {
        MarkSource::Row { row: r, field } => format!("{}.{}", row(*r), ident(&field.wire)),
        MarkSource::Input(i) => input_var(*i),
        MarkSource::Mark(k) => format!("{}.id", ident(&marks_in[*k].name)),
    }
}

fn lit_py(lit: &Lit, enums: &EnumMembers) -> String {
    match lit {
        Lit::Enum { ty, value } => format!("{ty}.{}", enums.member(ty, value)),
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
                "{}.{} != {enum_ty}.{}",
                row(*r),
                ident(&field.wire),
                enums.member(enum_ty, value)
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
            "{}.{} == {enum_ty}.{}",
            row(*r),
            ident(&field.wire),
            enums.member(enum_ty, value)
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
        } => format!("{}.{} {} {value}", row(*r), ident(&field.wire), op.symbol()),
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

fn print_validators(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
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
    super::actions::simplify(Expr::Not(Box::new(e.clone())))
}

// ── Rows and dispatch ────────────────────────────────────────────────────────

fn print_rows(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
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

fn print_dispatchers(domain: &DomainModel, enums: &EnumMembers, out: &mut String) {
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
                    format!("{} == {ty}.{}", param_name(n), enums.member(ty, v))
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
