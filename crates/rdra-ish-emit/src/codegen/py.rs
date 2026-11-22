//! The Python printer: `NewType` scalars and ids, `(str, Enum)` value sets, frozen
//! dataclasses, union state types, transition functions, rule checks, row conversions, and
//! dispatchers. Targets Python 3.10+ with standard library imports only.

mod runtime;

use runtime::{print_dispatchers, print_functions, print_rows, print_validators};
use std::collections::{BTreeMap, BTreeSet};

use super::actions::snake;
use super::ir::{
    Adapt, ArgCheck, Assigned, DomainModel, EnumValue, Expr, Field, FieldDefault, FieldType, Lit,
    MarkSource, OutKind, RecordKind, Scalar,
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

    let enums = EnumMembers::build(domain, options);
    let need_builtins = any_builtin_shadow(domain);
    let need_abc = any_rows_field(domain);
    let need_pydantic = !options.python.pydantic_records.is_empty();
    let need_dc = domain
        .records
        .iter()
        .any(|r| !options.python.pydantic_records.contains(&r.name))
        || !domain.sums.is_empty()
        || !domain.events.is_empty()
        || !domain.marks.is_empty()
        || !domain.fns.is_empty()
        || !domain.dispatchers.is_empty();
    let need_enum = domain
        .enums
        .iter()
        .any(|e| !enums.literals.contains(&e.name));
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
    out.push_str("import typing as _t\n");
    if need_pydantic {
        out.push_str("\nimport pydantic as _pydantic\n");
    }
    out.push('\n');

    out.push_str("# ── Scalars: replace with the project's own types if it has them ──\n\n");
    for s in Scalar::PRELUDE {
        if options.python.literal_value_sets
            && !domain
                .records
                .iter()
                .flat_map(|r| &r.fields)
                .chain(domain.sums.iter().flat_map(|s| &s.row_fields))
                .any(|f| uses_scalar(&f.ty, s))
        {
            continue;
        }
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
        if enums.literals.contains(&decl.name) {
            for line in &decl.doc {
                out.push_str(&comment(line, ""));
            }
            let values = decl
                .values
                .iter()
                .map(|v| string_literal(&v.wire))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("{} = _t.Literal[{values}]\n\n", decl.name));
            continue;
        }
        let base = if options.python.str_enum {
            "_enum.StrEnum"
        } else {
            "str, _enum.Enum"
        };
        out.push_str(&format!("class {}({base}):\n", decl.name));
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
        out.push_str("\n\n");
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
            let default_before_required = record.fields.iter().enumerate().any(|(i, f)| {
                f.default.is_some() && record.fields[i + 1..].iter().any(|f| f.default.is_none())
            });
            let pydantic = options.python.pydantic_records.contains(&record.name);
            let frozen = !options.python.mutable_records.contains(&record.name);
            if !pydantic {
                let mut args = vec![format!("frozen={}", if frozen { "True" } else { "False" })];
                if default_before_required {
                    args.push("kw_only=True".into());
                }
                if options.python.slots {
                    args.push("slots=True".into());
                }
                out.push_str(&format!("@_dc.dataclass({})\n", args.join(", ")));
            }
            let base = if pydantic {
                "(_pydantic.BaseModel)"
            } else {
                ""
            };
            out.push_str(&format!("class {}{base}:\n", record.name));
            let doc = docstring(&record.doc, "    ");
            if !doc.is_empty() {
                out.push_str(&doc);
            }
            if pydantic {
                out.push_str(&format!(
                    "    model_config = _pydantic.ConfigDict(extra=\"forbid\", frozen={})\n\n",
                    if frozen { "True" } else { "False" }
                ));
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
                    let default = match &field.default {
                        None => String::new(),
                        Some(FieldDefault::Null) => " = None".to_string(),
                        Some(FieldDefault::Bool(value)) => {
                            format!(" = {}", if *value { "True" } else { "False" })
                        }
                        Some(FieldDefault::Int(value)) => format!(" = {value}"),
                        Some(FieldDefault::ScalarText(value)) => {
                            format!(" = {}({})", field_type(&field.ty), string_literal(value))
                        }
                        Some(FieldDefault::Text(value)) => format!(" = {}", string_literal(value)),
                        Some(FieldDefault::Enum(value)) => {
                            let FieldType::Enum(name) = &field.ty else {
                                unreachable!("enum default has enum type")
                            };
                            format!(
                                " = {}",
                                enums.expression(name, &super::enum_value(value, None))
                            )
                        }
                        Some(FieldDefault::Empty) => match &field.ty {
                            FieldType::Tuple(_) => " = ()".to_string(),
                            _ if pydantic => " = _pydantic.Field(default_factory=list)".to_string(),
                            _ => " = _dc.field(default_factory=list)".to_string(),
                        },
                    };
                    out.push_str(&format!("    {}: {ty}{default}\n", ident(&field.wire)));
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
    literals: BTreeSet<String>,
}

impl EnumMembers {
    fn build(domain: &DomainModel, options: &Options) -> Self {
        let literals = domain
            .enums
            .iter()
            .filter(|decl| {
                options.python.literal_value_sets
                    && !options.python.enum_value_sets.contains(&decl.name)
                    && !domain
                        .sums
                        .iter()
                        .any(|s| matches!(&s.tag.ty, FieldType::Enum(name) if name == &decl.name))
            })
            .map(|decl| decl.name.clone())
            .collect();
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
        Self { members, literals }
    }

    fn expression(&self, enum_name: &str, value: &EnumValue) -> String {
        if self.literals.contains(enum_name) {
            string_literal(&value.wire)
        } else {
            format!("{enum_name}.{}", self.member(enum_name, value))
        }
    }

    fn member(&self, enum_name: &str, value: &EnumValue) -> String {
        self.members
            .get(&(enum_name.to_string(), value.wire.clone()))
            .cloned()
            .unwrap_or_else(|| snake(&value.ident).to_ascii_uppercase())
    }
}

fn any_builtin_shadow(domain: &DomainModel) -> bool {
    let has_shadow =
        |fields: &[Field], extra: Option<&str>| !field_shadows(fields, extra).is_empty();
    domain.records.iter().any(|r| has_shadow(&r.fields, None))
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
        FieldType::Enum(name) | FieldType::Id(name) | FieldType::Record(name) => name.clone(),
        FieldType::Rows(name) => format!("_abc.Sequence[{name}]"),
        FieldType::List(item) => format!("list[{}]", field_type(item)),
        FieldType::Tuple(item) => format!("tuple[{}, ...]", field_type(item)),
    }
}

fn field_type_in_class(ty: &FieldType, shadows: &BTreeSet<String>) -> String {
    match ty {
        FieldType::Scalar(s) => {
            let plain = scalar(*s, false);
            let shadowed = shadows.contains(plain);
            scalar(*s, shadowed).to_string()
        }
        FieldType::Enum(name) | FieldType::Id(name) | FieldType::Record(name) => name.clone(),
        FieldType::Rows(name) => format!("_abc.Sequence[{name}]"),
        FieldType::List(item) => format!("list[{}]", field_type(item)),
        FieldType::Tuple(item) => format!("tuple[{}, ...]", field_type(item)),
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
        return format!("{indent}#\n");
    }
    let prefix = format!("{indent}# ");
    let mut out = String::new();
    let mut current = prefix.clone();
    for word in line.split_whitespace() {
        if current.len() > prefix.len() && current.chars().count() + word.chars().count() + 1 > 96 {
            out.push_str(&current);
            out.push('\n');
            current = prefix.clone();
        }
        if current.len() > prefix.len() {
            current.push(' ');
        }
        current.push_str(word);
    }
    out.push_str(&current);
    out.push('\n');
    out
}

fn escape_docstring(text: &str) -> String {
    let mut s = text
        .trim_end()
        .replace('\\', "\\\\")
        .replace("\"\"\"", "\\\"\\\"\\\"");
    if s.ends_with('"') {
        s.push(' ');
    }
    s
}

fn docstring(lines: &[String], indent: &str) -> String {
    let lines: Vec<String> = lines
        .iter()
        .flat_map(|line| line.lines())
        .map(escape_docstring)
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

/// Inspect the typed IR so unused scalar declarations do not leak into a data-only module.
fn uses_scalar(ty: &FieldType, scalar: Scalar) -> bool {
    match ty {
        FieldType::Scalar(value) => *value == scalar,
        FieldType::List(item) | FieldType::Tuple(item) => uses_scalar(item, scalar),
        _ => false,
    }
}
