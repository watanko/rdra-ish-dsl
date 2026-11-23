//! Structured business decisions without choosing a persistence or execution mechanism.

use rdra_ish_core::model::{
    CheckExpr, CmpRhs, ColumnType, CrossCmpRhs, CrossConstraintScope, CrossEntityCondition,
    DerivedExpr, EffectValue, NodeRef, QualifiedModelColumnRef, QuantifierKind, RelKind, SetValue,
};
use rdra_ish_core::SemanticModel;
use serde::Serialize;

use super::{CheckStatus, Options};

mod flows;
mod python;
pub(super) use python::print_python;

#[derive(Clone, PartialEq, Eq, Serialize)]
struct Reference {
    entity: String,
    column: String,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
struct Operand {
    kind: String,
    reference: Option<Reference>,
    text: Option<String>,
    integer: Option<i64>,
    boolean: Option<bool>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
struct Comparison {
    left: Reference,
    operator: String,
    right: Vec<Operand>,
}

#[derive(Serialize)]
struct Quantifier {
    kind: String,
    entity: String,
    roles: Vec<String>,
    comparisons: Vec<Comparison>,
}

#[derive(Serialize)]
struct Condition {
    name: String,
    label: String,
    executable: bool,
    comparisons: Vec<Comparison>,
    quantifiers: Vec<Quantifier>,
    path: Vec<String>,
    roles: Vec<String>,
}

#[derive(Serialize)]
struct Transition {
    usecase: String,
    target: Reference,
    from: String,
    to: String,
    guard: Option<String>,
}

#[derive(Serialize)]
struct Effect {
    usecase: String,
    target: Reference,
    value: Operand,
    guard: Option<String>,
}

#[derive(Serialize)]
struct Guard {
    usecase: String,
    condition: String,
}

#[derive(Serialize)]
struct Variation {
    name: String,
    values: Vec<String>,
}

#[derive(Serialize)]
struct ForeignKey {
    child: String,
    parent: String,
    role: String,
    child_columns: Vec<String>,
    parent_columns: Vec<String>,
}

#[derive(Serialize)]
struct Expression {
    operator: String,
    comparison: Option<Comparison>,
    children: Vec<Expression>,
}

#[derive(Serialize)]
struct Field {
    reference: Reference,
    kind: String,
    nullable: bool,
    values: Vec<String>,
    derived: Option<Expression>,
    unsupported_derivation: Option<String>,
}

#[derive(Serialize)]
struct Policy {
    format_version: u8,
    source: String,
    fields: Vec<Field>,
    check_errors: Vec<String>,
    conditions: Vec<Condition>,
    transitions: Vec<Transition>,
    effects: Vec<Effect>,
    guards: Vec<Guard>,
    variations: Vec<Variation>,
    foreign_keys: Vec<ForeignKey>,
    flows: Vec<flows::BusinessFlow>,
}

pub(super) fn print(model: &SemanticModel, options: &Options) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&build(model, options))
}

fn build(model: &SemanticModel, options: &Options) -> Policy {
    let condition_name = |key| model.conditions[key].id.clone();
    let fields = model
        .entities
        .iter()
        .flat_map(|(entity, definition)| {
            definition.columns.iter().map(move |column| Field {
                reference: Reference {
                    entity: definition.id.clone(),
                    column: column.name.clone(),
                },
                kind: match &column.col_type {
                    ColumnType::Int => "integer",
                    ColumnType::Bool => "boolean",
                    ColumnType::Money | ColumnType::Decimal => "decimal",
                    ColumnType::Date => "date",
                    ColumnType::DateTime => "datetime",
                    ColumnType::Enum(_) | ColumnType::Variation(_) => "enum",
                    ColumnType::String => "string",
                    ColumnType::Record(_) => "record",
                    ColumnType::List(_) | ColumnType::Tuple(_) => "collection",
                }
                .into(),
                nullable: column.is_nullable,
                values: model
                    .column_values(column)
                    .unwrap_or_default()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                unsupported_derivation: match &column.derived_expr {
                    Some(value @ DerivedExpr::Arith(_)) => Some(value.display()),
                    _ => None,
                },
                derived: match &column.derived_expr {
                    Some(DerivedExpr::Logical(expression)) => {
                        Some(logical(model, entity, expression))
                    }
                    _ => None,
                },
            })
        })
        .collect();
    let conditions = model
        .conditions
        .values()
        .map(|condition| {
            let (comparisons, quantifiers, path, roles) = match &condition.body {
                Some(body) => {
                    let path = match &body.along {
                        CrossConstraintScope::GlobalProduct => Vec::new(),
                        CrossConstraintScope::RelationPath { path, .. } => path
                            .iter()
                            .map(|entity| model.entities[*entity].id.clone())
                            .collect(),
                    };
                    let quantifiers = body
                        .quantifiers
                        .iter()
                        .map(|quantifier| Quantifier {
                            kind: match quantifier.kind {
                                QuantifierKind::Has => "has",
                                QuantifierKind::None => "none",
                            }
                            .into(),
                            entity: model.entities[quantifier.related].id.clone(),
                            roles: quantifier.by.clone(),
                            comparisons: quantifier
                                .conditions
                                .iter()
                                .map(|item| comparison(model, item))
                                .collect(),
                        })
                        .collect();
                    (
                        body.conds
                            .iter()
                            .map(|item| comparison(model, item))
                            .collect(),
                        quantifiers,
                        path,
                        body.by.clone(),
                    )
                }
                None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
            };
            Condition {
                name: condition.id.clone(),
                label: condition.label.clone(),
                executable: condition.body.is_some(),
                comparisons,
                quantifiers,
                path,
                roles,
            }
        })
        .collect();
    let transitions = model
        .state_transitions
        .iter()
        .map(|transition| Transition {
            usecase: model.use_cases[transition.usecase].id.clone(),
            target: Reference {
                entity: model.entities[transition.entity].id.clone(),
                column: transition.column.clone(),
            },
            from: transition.from.clone(),
            to: transition.to.clone(),
            guard: transition.guard.map(condition_name),
        })
        .collect();
    let effects = model
        .column_effects
        .iter()
        .map(|effect| Effect {
            usecase: model.use_cases[effect.origin].id.clone(),
            target: Reference {
                entity: model.entities[effect.entity].id.clone(),
                column: effect.column.clone(),
            },
            value: match &effect.value {
                SetValue::Value(value) => literal(value),
                SetValue::Copy(source) => Operand {
                    kind: "column".into(),
                    reference: Some(reference(model, source)),
                    ..Operand::default()
                },
            },
            guard: effect.guard.map(condition_name),
        })
        .collect();
    let guards = model
        .relations
        .iter()
        .filter_map(|relation| {
            if relation.kind != RelKind::AppliesTo {
                return None;
            }
            match (&relation.from, &relation.to) {
                (NodeRef::Condition(condition), NodeRef::UseCase(usecase)) => Some(Guard {
                    usecase: model.use_cases[*usecase].id.clone(),
                    condition: condition_name(*condition),
                }),
                _ => None,
            }
        })
        .collect();
    let variations = model
        .variations
        .values()
        .map(|variation| Variation {
            name: variation.id.clone(),
            values: variation
                .values
                .iter()
                .map(|value| value.value.clone())
                .collect(),
        })
        .collect();
    let foreign_keys = model
        .foreign_keys()
        .map(|(_, key)| ForeignKey {
            child: model.entities[key.child].id.clone(),
            parent: model.entities[key.parent].id.clone(),
            role: key.role_or(&model.entities[key.parent].id).to_string(),
            child_columns: key.columns.iter().map(|(child, _)| child.clone()).collect(),
            parent_columns: key
                .columns
                .iter()
                .map(|(_, parent)| parent.clone())
                .collect(),
        })
        .collect();
    let check_errors = match &options.check {
        CheckStatus::Passed { .. } => Vec::new(),
        CheckStatus::Failed { errors } => errors.clone(),
    };
    Policy {
        flows: flows::build(model),
        format_version: 1,
        source: options.model_path.clone(),
        fields,
        check_errors,
        conditions,
        transitions,
        effects,
        guards,
        variations,
        foreign_keys,
    }
}

fn logical(
    model: &SemanticModel,
    entity: rdra_ish_core::model::EntityKey,
    expression: &CheckExpr,
) -> Expression {
    match expression {
        CheckExpr::Not(value) => Expression {
            operator: "not".into(),
            comparison: None,
            children: vec![logical(model, entity, value)],
        },
        CheckExpr::And(left, right) | CheckExpr::Or(left, right) => Expression {
            operator: if matches!(expression, CheckExpr::And(..)) {
                "and"
            } else {
                "or"
            }
            .into(),
            comparison: None,
            children: vec![logical(model, entity, left), logical(model, entity, right)],
        },
        CheckExpr::Cmp(value) => {
            let right = match &value.rhs {
                CmpRhs::Column(column) => vec![Operand {
                    kind: "column".into(),
                    reference: Some(Reference {
                        entity: model.entities[entity].id.clone(),
                        column: column.clone(),
                    }),
                    ..Operand::default()
                }],
                CmpRhs::IntLit(value) => vec![Operand {
                    kind: "integer".into(),
                    integer: Some(*value),
                    ..Operand::default()
                }],
                CmpRhs::Now => vec![Operand {
                    kind: "clock".into(),
                    ..Operand::default()
                }],
                CmpRhs::Value(value) => vec![literal(value)],
                CmpRhs::Values(values) => values.iter().map(literal).collect(),
            };
            Expression {
                operator: "comparison".into(),
                comparison: Some(Comparison {
                    left: Reference {
                        entity: model.entities[entity].id.clone(),
                        column: value.lhs_column.clone(),
                    },
                    operator: value.op.as_str().into(),
                    right,
                }),
                children: Vec::new(),
            }
        }
    }
}

fn reference(model: &SemanticModel, column: &QualifiedModelColumnRef) -> Reference {
    Reference {
        entity: model.entities[column.entity].id.clone(),
        column: column.column.clone(),
    }
}

fn literal(value: &EffectValue) -> Operand {
    let mut operand = Operand::default();
    match value {
        EffectValue::EnumVariant(value) => {
            operand.kind = "text".into();
            operand.text = Some(value.clone());
        }
        EffectValue::Bool(value) => {
            operand.kind = "boolean".into();
            operand.boolean = Some(*value);
        }
        EffectValue::Int(value) => {
            operand.kind = "integer".into();
            operand.integer = Some(*value);
        }
        EffectValue::Null => operand.kind = "null".into(),
        EffectValue::Present => operand.kind = "present".into(),
    }
    operand
}

fn comparison(model: &SemanticModel, condition: &CrossEntityCondition) -> Comparison {
    match condition {
        CrossEntityCondition::Equals { column, value } => Comparison {
            left: reference(model, column),
            operator: "==".into(),
            right: vec![literal(value)],
        },
        CrossEntityCondition::Comparison(comparison) => {
            let right = match &comparison.rhs {
                CrossCmpRhs::Column(column) => vec![Operand {
                    kind: "column".into(),
                    reference: Some(reference(model, column)),
                    ..Operand::default()
                }],
                CrossCmpRhs::IntLit(value) => vec![Operand {
                    kind: "integer".into(),
                    integer: Some(*value),
                    ..Operand::default()
                }],
                CrossCmpRhs::Now => vec![Operand {
                    kind: "clock".into(),
                    ..Operand::default()
                }],
                CrossCmpRhs::Value(value) => vec![literal(value)],
                CrossCmpRhs::Values(values) => values.iter().map(literal).collect(),
            };
            Comparison {
                left: reference(model, &comparison.lhs),
                operator: comparison.op.as_str().into(),
                right,
            }
        }
    }
}
