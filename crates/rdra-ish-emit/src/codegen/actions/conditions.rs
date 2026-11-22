//! Runtime expressions for modeled conditions.

use super::*;

/// A predicate on the rows `bind` names. `quant` lets it range over related rows.
///
/// The row `at` names: a function row by `bind`, or the row of an enclosing quantifier.
pub(super) fn resolve<'d>(
    at: At,
    rows: &[RowParam],
    bind: Bind<'_, 'd>,
    quant: &Option<&mut Quant<'_>>,
) -> Option<(usize, Option<&'d VariantDecl>, RowParam)> {
    if let (At::Bound(k), Some(q)) = (at, quant.as_deref()) {
        let k = usize::from(k);
        if k >= q.base {
            let elem = *q.stack.get(k - q.base)?;
            return Some((elem, None, q.elems[elem - q.n_rows].clone()));
        }
    }
    let (r, state) = bind(at)?;
    Some((r, state, rows.get(r)?.clone()))
}

pub(super) fn atom(
    ts: &TransitionSystem,
    lhs: &Term,
    op: TsCmp,
    rhs: &Term,
    rows: &[RowParam],
    bind: Bind<'_, '_>,
    quant: &mut Option<&mut Quant<'_>>,
) -> Result<Expr, String> {
    let unsupported = || "a condition atom this phase does not print".to_string();
    // A key between two rows: the child's key names the parent.
    let key = match (lhs, op, rhs) {
        (Term::Slot { var, at }, TsCmp::Eq, Term::Index(parent))
        | (Term::Index(parent), TsCmp::Eq, Term::Slot { var, at }) => match &ts.var(*var).kind {
            VarKind::Owner { fk, .. } => Some((*fk, *at, *parent)),
            _ => None,
        },
        _ => None,
    };
    if let Some((fk, child_at, parent_at)) = key {
        let q = quant.as_deref().ok_or_else(unsupported)?;
        let fk = q.model.foreign_key(fk);
        let [(child_col, parent_col)] = fk.columns.as_slice() else {
            return Err("a key over several columns".into());
        };
        let quant_ref: &Option<&mut Quant<'_>> = quant;
        let (c, _, crow) = resolve(child_at, rows, bind, quant_ref).ok_or_else(unsupported)?;
        let (p, _, prow) = resolve(parent_at, rows, bind, quant_ref).ok_or_else(unsupported)?;
        let child_field = crow.fields.iter().find(|f| f.wire == *child_col);
        let parent_field = prow.fields.iter().find(|f| f.wire == *parent_col);
        let (Some(child_field), Some(parent_field)) = (child_field, parent_field) else {
            return Err("a key on a column with no field".into());
        };
        return Ok(Expr::KeyEq {
            child: c,
            child_field: child_field.clone(),
            parent: p,
            parent_field: parent_field.clone(),
        });
    }
    let Term::Slot { var, at } = lhs else {
        return Err(unsupported());
    };
    let VarKind::Column { column, .. } = &ts.var(*var).kind else {
        return Err("a condition on a comparison proposition (phase 5)".into());
    };
    let quant_ref: &Option<&mut Quant<'_>> = quant;
    let (r, state, row) = resolve(*at, rows, bind, quant_ref).ok_or_else(unsupported)?;
    let domain = &ts.var(*var).domain;
    let negate = |e: Expr| match op {
        TsCmp::Eq => Ok(e),
        TsCmp::Ne => Ok(Expr::Not(Box::new(e))),
        _ => Err(unsupported()),
    };
    if matches!(domain, Domain::NullableInt) && matches!(rhs, Term::Lit(Value::NULL)) {
        let is_null = match row.fields.iter().find(|f| f.wire == *column) {
            None if row.always_null.contains(column) => Expr::True,
            Some(field) if !field.nullable => Expr::False,
            Some(field) => Expr::IsNull {
                row: r,
                field: field.clone(),
            },
            None => return Err(unsupported()),
        };
        return negate(is_null);
    }
    if let (
        Domain::Int | Domain::NullableInt,
        Term::Slot {
            var: right,
            at: right_at,
        },
    ) = (domain, rhs)
    {
        let VarKind::Column {
            column: right_column,
            ..
        } = &ts.var(*right).kind
        else {
            return Err(unsupported());
        };
        let (right, _, right_row) =
            resolve(*right_at, rows, bind, quant_ref).ok_or_else(unsupported)?;
        let left_field = row.fields.iter().find(|field| field.wire == *column);
        let right_field = right_row
            .fields
            .iter()
            .find(|field| field.wire == *right_column);
        let (Some(left_field), Some(right_field)) = (left_field, right_field) else {
            if left_field.is_none() && !row.always_null.contains(column)
                || right_field.is_none() && !right_row.always_null.contains(right_column)
            {
                return Err(unsupported());
            }
            let equal = match (left_field, right_field) {
                (None, None) => Expr::True,
                (Some(field), None) if field.nullable => Expr::IsNull {
                    row: r,
                    field: field.clone(),
                },
                (None, Some(field)) if field.nullable => Expr::IsNull {
                    row: right,
                    field: field.clone(),
                },
                _ => Expr::False,
            };
            return match op {
                TsCmp::Eq => Ok(equal),
                TsCmp::Ne => Ok(Expr::Not(Box::new(equal))),
                _ => Ok(Expr::False),
            };
        };
        if left_field.ty != FieldType::Scalar(super::ir::Scalar::Int)
            || right_field.ty != left_field.ty
        {
            return Err("a comparison between columns that are not plain Int values".into());
        }
        return Ok(Expr::FieldCmp {
            left: r,
            left_field: left_field.clone(),
            op: comparison_operator(op),
            right,
            right_field: right_field.clone(),
        });
    }
    match (domain, rhs) {
        (Domain::Enum(values), Term::Lit(v)) => {
            let wire = values.get(usize::from(v.0)).ok_or_else(unsupported)?;
            // The tag of a state type is fixed by the source state.
            if let Some(variant) = state {
                if !row.fields.iter().any(|f| f.wire == *column) {
                    let holds = variant.value.wire == *wire;
                    return negate(if holds { Expr::True } else { Expr::False });
                }
            }
            let field = row
                .fields
                .iter()
                .find(|f| f.wire == *column)
                .ok_or_else(unsupported)?;
            let FieldType::Enum(enum_ty) = &field.ty else {
                return Err(unsupported());
            };
            if field.nullable {
                return Err("a comparison on a nullable enum column".into());
            }
            negate(Expr::EnumEq {
                row: r,
                field: field.clone(),
                enum_ty: enum_ty.clone(),
                value: EnumValue {
                    ident: type_ident(wire),
                    wire: wire.clone(),
                    label: None,
                },
            })
        }
        (Domain::Bool, Term::Lit(v)) => {
            let field = row
                .fields
                .iter()
                .find(|f| f.wire == *column)
                .ok_or_else(unsupported)?;
            if field.nullable {
                return Err("a comparison on a nullable Bool column".into());
            }
            negate(Expr::BoolEq {
                row: r,
                field: field.clone(),
                value: *v == Value::TRUE,
            })
        }
        (Domain::Nullable, Term::Lit(v)) => {
            let is_null = match row.fields.iter().find(|f| f.wire == *column) {
                None if row.always_null.contains(column) => Expr::True,
                None => return Err(unsupported()),
                Some(f) if !f.nullable => Expr::False,
                Some(f) => Expr::IsNull {
                    row: r,
                    field: f.clone(),
                },
            };
            let e = if *v == Value::NULL {
                is_null
            } else {
                Expr::Not(Box::new(is_null))
            };
            negate(e)
        }
        (Domain::Int | Domain::NullableInt, Term::Int(_) | Term::Lit(_)) => {
            let field = row
                .fields
                .iter()
                .find(|f| f.wire == *column)
                .ok_or_else(unsupported)?;
            if field.ty != FieldType::Scalar(super::ir::Scalar::Int) {
                return Err("a comparison on a column that is not a plain Int".into());
            }
            let value = match rhs {
                Term::Int(n) => *n,
                Term::Lit(v) => domain.number(*v).ok_or_else(unsupported)?,
                _ => unreachable!(),
            };
            let op = match op {
                TsCmp::Eq => CmpOp::Eq,
                TsCmp::Ne => CmpOp::Ne,
                TsCmp::Lt => CmpOp::Lt,
                TsCmp::Le => CmpOp::Le,
                TsCmp::Gt => CmpOp::Gt,
                TsCmp::Ge => CmpOp::Ge,
            };
            Ok(Expr::IntCmp {
                row: r,
                field: field.clone(),
                op,
                value,
            })
        }
        _ => Err(unsupported()),
    }
}

fn comparison_operator(op: TsCmp) -> CmpOp {
    match op {
        TsCmp::Eq => CmpOp::Eq,
        TsCmp::Ne => CmpOp::Ne,
        TsCmp::Lt => CmpOp::Lt,
        TsCmp::Le => CmpOp::Le,
        TsCmp::Gt => CmpOp::Gt,
        TsCmp::Ge => CmpOp::Ge,
    }
}
