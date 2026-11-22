//! Boolean and numeric expression rendering for Rust domains.

use super::*;
use crate::codegen::ir::CmpOp;

pub(super) fn expr(
    e: &super::ir::Expr,
    row: &dyn Fn(usize) -> String,
    input: &dyn Fn(usize) -> String,
) -> String {
    use super::ir::Expr;
    match e {
        Expr::True => "true".to_string(),
        Expr::False => "false".to_string(),
        Expr::Not(inner) => format!("!({})", expr(inner, row, input)),
        Expr::And(items) if items.is_empty() => "true".to_string(),
        Expr::Or(items) if items.is_empty() => "false".to_string(),
        Expr::And(items) => items
            .iter()
            .map(|i| format!("({})", expr(i, row, input)))
            .collect::<Vec<_>>()
            .join(" && "),
        Expr::Or(items) => items
            .iter()
            .map(|i| format!("({})", expr(i, row, input)))
            .collect::<Vec<_>>()
            .join(" || "),
        Expr::IsNull { row: r, field } => format!("{}.{}.is_none()", row(*r), ident(&field.wire)),
        Expr::EnumEq {
            row: r,
            field,
            enum_ty,
            value,
        } => format!(
            "{}.{} == {enum_ty}::{}",
            row(*r),
            ident(&field.wire),
            variant_ident(&value.ident)
        ),
        Expr::BoolEq {
            row: r,
            field,
            value,
        } => {
            format!("{}.{} == {value}", row(*r), ident(&field.wire))
        }
        Expr::IntCmp {
            row: r,
            field,
            op,
            value,
        } => {
            let at = format!("{}.{}", row(*r), ident(&field.wire));
            if field.nullable && matches!(op, CmpOp::Eq | CmpOp::Ne) {
                format!("{at} {} ::core::option::Option::Some({value})", op.symbol())
            } else if field.nullable {
                format!("{at}.is_some_and(|value| value {} {value})", op.symbol())
            } else {
                format!("{at} {} {value}", op.symbol())
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
            if !left_field.nullable && !right_field.nullable {
                format!("{left_at} {} {right_at}", op.symbol())
            } else {
                let l = if left_field.nullable {
                    format!("{left_at}.as_ref()")
                } else {
                    format!("::core::option::Option::Some(&{left_at})")
                };
                let r = if right_field.nullable {
                    format!("{right_at}.as_ref()")
                } else {
                    format!("::core::option::Option::Some(&{right_at})")
                };
                if matches!(op, CmpOp::Eq | CmpOp::Ne) {
                    return format!("{l} {} {r}", op.symbol());
                }
                format!("::core::matches!(({l}, {r}), (::core::option::Option::Some(left), ::core::option::Option::Some(right)) if left {} right)", op.symbol())
            }
        }
        Expr::KeyEq {
            child,
            child_field,
            parent,
            parent_field,
        } => {
            let c = format!("{}.{}", row(*child), ident(&child_field.wire));
            let p = format!("{}.{}", row(*parent), ident(&parent_field.wire));
            if child_field.nullable {
                format!("{c}.as_ref() == ::core::option::Option::Some(&{p})")
            } else {
                format!("{c} == {p}")
            }
        }
        Expr::Exists {
            input: i,
            elem,
            body,
        } => format!(
            "{}.iter().any(|{}| {})",
            input(*i),
            row(*elem),
            expr(body, row, input)
        ),
        Expr::Forall {
            input: i,
            elem,
            body,
        } => format!(
            "{}.iter().all(|{}| {})",
            input(*i),
            row(*elem),
            expr(body, row, input)
        ),
    }
}
