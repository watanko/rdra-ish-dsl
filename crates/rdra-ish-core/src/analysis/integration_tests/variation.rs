use crate::analysis::build_model;
use crate::diagnostics::RdraError;
use crate::lifecycle::{derive_states, AxisKind, Caps, Scope};
use crate::model::{ColumnType, EffectValue, SemanticModel};
use rdra_ish_syntax::parse;

fn build(src: &str) -> (SemanticModel, Vec<crate::Diagnostic>) {
    let (ast, errors) = parse(src);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    build_model(&ast)
}

fn errors(diags: &[crate::Diagnostic]) -> Vec<&RdraError> {
    diags
        .iter()
        .filter(|d| !d.is_warning)
        .map(|d| &d.error)
        .collect()
}

const PAYMENT: &str = r#"
variation PaymentMethod "Payment Method" {
  cash "Cash"
  card
}
entity Payment "Payment" {
  id: Int @pk
  method: PaymentMethod @default(card)
  status: Enum(pending, posted) @default(pending)
}
"#;

#[test]
fn variation_typed_column_resolves_to_the_variation_and_its_values() {
    let (model, diags) = build(PAYMENT);
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let entity = model.entities.values().next().unwrap();
    let method = entity.columns.iter().find(|c| c.name == "method").unwrap();
    assert!(matches!(method.col_type, ColumnType::Variation(_)));
    assert_eq!(model.column_values(method), Some(vec!["cash", "card"]));
    let status = entity.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(model.column_values(status), Some(vec!["pending", "posted"]));
    let id = entity.columns.iter().find(|c| c.name == "id").unwrap();
    assert_eq!(model.column_values(id), None);
}

#[test]
fn column_typed_by_an_undeclared_name_is_unknown_column_type() {
    let (_, diags) = build("entity E \"E\" { id: Int @pk\n method: PaymentMethd }\n");
    let errs = errors(&diags);
    assert!(
        errs.iter().any(|e| matches!(e,
            RdraError::UnknownColumnType { entity, col, name }
                if entity == "E" && col == "method" && name == "PaymentMethd")),
        "{errs:?}"
    );
}

#[test]
fn default_outside_the_variation_values_is_rejected() {
    let src = PAYMENT.replace("@default(card)", "@default(crypto)");
    let (_, diags) = build(&src);
    let errs = errors(&diags);
    assert!(
        errs.iter().any(|e| matches!(e,
            RdraError::InvalidVariationValue { variation, value, .. }
                if variation == "PaymentMethod" && value == "crypto")),
        "{errs:?}"
    );
}

#[test]
fn sets_value_is_checked_against_the_variation_values() {
    let ok = format!("{PAYMENT}usecase Pay \"Pay\"\nsets(Pay, Payment.method == cash)\n");
    let (model, diags) = build(&ok);
    assert!(errors(&diags).is_empty(), "{diags:?}");
    assert!(model.column_effects.iter().any(|e| e.column == "method"
        && e.value.literal() == Some(&EffectValue::EnumVariant("cash".into()))));

    let bad = format!("{PAYMENT}usecase Pay \"Pay\"\nsets(Pay, Payment.method == crypto)\n");
    let (_, diags) = build(&bad);
    let errs = errors(&diags);
    assert!(
        errs.iter().any(|e| matches!(e,
            RdraError::InvalidVariationValue { value, allowed, .. }
                if value == "crypto" && allowed == "cash, card")),
        "{errs:?}"
    );
}

#[test]
fn transitions_on_a_variation_column_is_an_error() {
    let src =
        format!("{PAYMENT}usecase Pay \"Pay\"\ntransitions(Pay, Payment.method, cash -> card)\n");
    let (_, diags) = build(&src);
    let errs = errors(&diags);
    assert!(
        errs.iter().any(
            |e| matches!(e, RdraError::TransitionsColumnNotEnum { col, .. } if col == "method")
        ),
        "{errs:?}"
    );
}

#[test]
fn variation_column_adds_no_state_axis_while_an_inline_enum_does() {
    let lifecycle = r#"
usecase Pay "Pay"
transitions(Pay, Payment.status, pending -> posted)
"#;
    let as_variation = format!("{PAYMENT}{lifecycle}").replace(
        "status: Enum",
        "backup_method: PaymentMethod @null\n  status: Enum",
    );
    let (model, diags) = build(&as_variation);
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let result = &derive_states(&model, &Scope::ALL, Caps::STATES).views[0];
    let columns: Vec<_> = result.axes.iter().map(|a| a.column.as_str()).collect();
    assert_eq!(columns, vec!["status"], "{:?}", result.axes);
    assert!(result
        .axes
        .iter()
        .all(|a| matches!(a.kind, AxisKind::Enum(_))));

    let as_enum = as_variation.replace(
        "method: PaymentMethod @default(card)",
        "method: Enum(cash, card) @default(card)",
    );
    let (model, diags) = build(&as_enum);
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let result = &derive_states(&model, &Scope::ALL, Caps::STATES).views[0];
    let mut columns: Vec<_> = result.axes.iter().map(|a| a.column.as_str()).collect();
    columns.sort();
    assert_eq!(columns, vec!["method", "status"]);
}

#[test]
fn dto_fields_resolve_forward_records_and_ordered_collections() {
    let (model, diags) = build(
        r#"
        dto Envelope "Envelope" {
            user: User
            users: List(User) @default(empty)
            tags: Tuple(String)
        }
        dto User "User" { name: String }
    "#,
    );
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let envelope = model.dtos.values().find(|d| d.id == "Envelope").unwrap();
    assert!(matches!(envelope.fields[0].col_type, ColumnType::Record(_)));
    assert!(
        matches!(&envelope.fields[1].col_type, ColumnType::List(item) if matches!(**item, ColumnType::Record(_)))
    );
    assert!(
        matches!(&envelope.fields[2].col_type, ColumnType::Tuple(item) if **item == ColumnType::String)
    );
}

#[test]
fn dto_collections_reject_unknown_elements_and_entity_embedding() {
    for src in [
        r#"dto Envelope "E" { users: List(Missing) }"#,
        r#"dto User "U" { name: String } entity E "E" { user: User }"#,
    ] {
        let (_, diags) = build(src);
        assert!(
            errors(&diags)
                .iter()
                .any(|e| matches!(e, RdraError::UnknownColumnType { .. })),
            "{diags:?}"
        );
    }
}

#[test]
fn a_quoted_variation_wire_value_and_nullable_default_remain_distinct() {
    let (model, diags) = build(
        r#"
        variation Model "Model" { "model-3.1" "model-3.8" }
        dto Settings "Settings" { selected: Model @default(null) @null }
    "#,
    );
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let variation = model.variations.values().next().unwrap();
    assert_eq!(
        variation
            .values
            .iter()
            .map(|value| value.value.as_str())
            .collect::<Vec<_>>(),
        ["model-3.1", "model-3.8"]
    );
    let field = &model.dtos.values().next().unwrap().fields[0];
    assert_eq!(field.default_val.as_deref(), Some("null"));
    assert!(!field.default_quoted);
}
