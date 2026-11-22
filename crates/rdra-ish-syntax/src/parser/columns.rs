//! Column types, annotations, and constructor defaults.

use super::*;

// ── Column annotations ────────────────────────────────────────────────────────

/// An ident, string-lit, or int-lit used as an annotation argument value.
fn ann_value() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    let int_lit = select! { Token::IntLit(s) => s };
    ident().or(string_lit()).or(int_lit)
}

fn annotation() -> impl Parser<Token, Annotation, Error = Simple<Token>> + Clone {
    let annotation_args = || {
        just(Token::LParen)
            .ignore_then(ident().separated_by(just(Token::Comma)).allow_trailing())
            .then_ignore(just(Token::RParen))
            .or_not()
    };

    // @pk  or  @pk(a, b)
    let at_pk = just(Token::AtPk)
        .ignore_then(annotation_args())
        .map(|args| match args {
            None => Annotation::Pk,
            Some(v) if v.is_empty() => Annotation::Pk,
            Some(v) => Annotation::PkComposite(v),
        });

    // @unique  or  @unique(a, b)
    let at_unique = just(Token::AtUnique)
        .ignore_then(annotation_args())
        .map(|args| match args {
            None => Annotation::Unique,
            Some(v) if v.is_empty() => Annotation::Unique,
            Some(v) => Annotation::UniqueComposite(v),
        });

    // @index  or  @index(a, b)
    let at_index = just(Token::AtIndex)
        .ignore_then(annotation_args())
        .map(|args| match args {
            None => Annotation::Index,
            Some(v) if v.is_empty() => Annotation::Index,
            Some(v) => Annotation::IndexComposite(v),
        });

    let at_check = expr_annotation(Token::AtCheck, "@check", Annotation::Check);

    // @null
    let at_null = just(Token::AtNull).to(Annotation::Null);

    // `now()` / `uuid()`: a function the database calls.
    let call = || {
        ident()
            .or(just(Token::Now).to("now".to_string()))
            .then_ignore(just(Token::LParen))
            .then_ignore(just(Token::RParen))
    };

    // @default(value) or @default(now())
    let at_default = just(Token::AtDefault)
        .ignore_then(just(Token::LParen))
        .ignore_then(
            call()
                .map(Annotation::DefaultCall)
                .or(string_lit().map(|value| Annotation::Default {
                    value,
                    quoted: true,
                }))
                .or(ann_value().map(|value| Annotation::Default {
                    value,
                    quoted: false,
                })),
        )
        .then_ignore(just(Token::RParen));

    // @generated(identity)
    let at_generated = just(Token::AtGenerated)
        .ignore_then(just(Token::LParen))
        .ignore_then(ident())
        .then_ignore(just(Token::RParen))
        .map(Annotation::Generated);

    // @on_update(now())
    let at_on_update = just(Token::AtOnUpdate)
        .ignore_then(just(Token::LParen))
        .ignore_then(call())
        .then_ignore(just(Token::RParen))
        .map(Annotation::OnUpdate);

    // @label("...")
    let at_label = just(Token::AtLabel)
        .ignore_then(just(Token::LParen))
        .ignore_then(string_lit())
        .then_ignore(just(Token::RParen))
        .map(Annotation::Label);

    let at_soft_delete = just(Token::AtSoftDelete).to(Annotation::SoftDelete);
    let at_history = just(Token::AtHistory).to(Annotation::History);
    let at_tenant = just(Token::AtTenant).to(Annotation::Tenant);
    let at_derived = expr_annotation(Token::AtDerived, "@derived", Annotation::Derived);
    let at_version = just(Token::AtVersion).to(Annotation::Version);

    // @end(a, b): at least one value
    let at_end = just(Token::AtEnd)
        .ignore_then(just(Token::LParen))
        .ignore_then(ident().separated_by(just(Token::Comma)).allow_trailing())
        .then_ignore(just(Token::RParen))
        .try_map(|values: Vec<String>, span| {
            if values.is_empty() {
                Err(Simple::custom(
                    span,
                    "`@end` takes at least one value: `@end(value)`",
                ))
            } else {
                Ok(Annotation::End(values))
            }
        });

    choice((
        at_pk,
        at_unique,
        at_index,
        at_check,
        at_null,
        at_default,
        at_label,
        at_soft_delete,
        at_history,
        at_tenant,
        at_derived,
        at_end,
        at_generated,
        at_on_update,
        at_version,
    ))
}

/// `@check(expr)` / `@derived(expr)`. The quoted form of earlier versions is an error whose
/// message is the unquoted rewrite.
fn expr_annotation(
    token: Token,
    name: &'static str,
    wrap: fn(ColumnExpr) -> Annotation,
) -> impl Parser<Token, Annotation, Error = Simple<Token>> + Clone {
    just(token)
        .ignore_then(just(Token::LParen))
        .ignore_then(string_lit().map(Err).or(column_expr().map(Ok)))
        .then_ignore(just(Token::RParen))
        .try_map(move |expr, span| match expr {
            Ok(expr) => Ok(wrap(expr)),
            Err(quoted) => Err(Simple::custom(
                span,
                format!("`{name}` takes an unquoted expression; write `{name}({quoted})`"),
            )),
        })
}

/// `operand (+|-|*) operand` over a column or an integer, or a logical expression.
fn column_expr() -> impl Parser<Token, ColumnExpr, Error = Simple<Token>> + Clone {
    let arith_operand =
        select! { Token::IntLit(s) => Operand::IntLit(s) }.or(column_name().map(Operand::Column));
    let arith_op = select! {
        Token::Plus => ArithOp::Add,
        Token::Minus => ArithOp::Sub,
        Token::Star => ArithOp::Mul,
    };
    let arith = arith_operand
        .clone()
        .then(arith_op)
        .then(arith_operand)
        .map_with_span(|((lhs, op), rhs), span| ColumnExpr::Arith(Arith { lhs, op, rhs, span }));
    arith.or(logical_expr().map(ColumnExpr::Logical))
}

// ── Column type ───────────────────────────────────────────────────────────────

fn col_type() -> impl Parser<Token, ColType, Error = Simple<Token>> + Clone {
    let simple = select! {
        Token::TInt      => ColType::Int,
        Token::TString   => ColType::String,
        Token::TMoney    => ColType::Money,
        Token::TDateTime => ColType::DateTime,
        Token::TDate     => ColType::Date,
        Token::TBool     => ColType::Bool,
        Token::TDecimal  => ColType::Decimal,
    };

    // Enum(active "販売中", discontinued) — at least one variant required, labels optional
    let enum_value = path_ident()
        .then(string_lit().or_not())
        .map(|(value, label)| EnumValue { value, label });
    let enum_ty = just(Token::TEnum)
        .ignore_then(just(Token::LParen))
        .ignore_then(enum_value.separated_by(just(Token::Comma)).allow_trailing())
        .then_ignore(just(Token::RParen))
        .try_map(|variants: Vec<EnumValue>, span| {
            if variants.is_empty() {
                Err(Simple::custom(span, "Enum() requires at least one variant"))
            } else {
                Ok(ColType::Enum(variants))
            }
        });

    let named = ident().map(ColType::Named);

    enum_ty.or(simple).or(named)
}

// ── Column definition ─────────────────────────────────────────────────────────

/// A column name or value: an identifier, or any kind keyword.
///
/// Columns and values never share a position with a declaration, so a word such as
/// `goal` or `field` is unambiguous where a column or a value is expected.
pub(super) fn column_name() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    path_ident()
}

/// `cash "Cash"` or `adjustment` (label optional)
pub(super) fn variation_value() -> impl Parser<Token, VariationValue, Error = Simple<Token>> + Clone
{
    path_ident()
        .then(string_lit().or_not())
        .map_with_span(|(value, label), span| VariationValue { value, label, span })
        .or(string_lit().try_map(|value, span| {
            if value_needs_quotes(&value) {
                Ok(VariationValue {
                    value,
                    label: None,
                    span,
                })
            } else {
                Err(Simple::custom(
                    span,
                    "a variation identifier is a bare token; quote only wire values that need it",
                ))
            }
        }))
}

/// `name: Type @ann1 @ann2 ...`
pub(super) fn column() -> impl Parser<Token, Column, Error = Simple<Token>> + Clone {
    let type_arg = select! {
        Token::IntLit(s) => TypeArg::Int(s),
        Token::TString => TypeArg::Ident("String".into()),
        Token::TInt => TypeArg::Ident("Int".into()),
        Token::TBool => TypeArg::Ident("Bool".into()),
        Token::TDateTime => TypeArg::Ident("DateTime".into()),
        Token::TDate => TypeArg::Ident("Date".into()),
        Token::TDecimal => TypeArg::Ident("Decimal".into()),
        Token::TMoney => TypeArg::Ident("Money".into()),
    }
    .or(ident().map(TypeArg::Ident));
    let type_args = just(Token::LParen)
        .ignore_then(type_arg.separated_by(just(Token::Comma)))
        .then_ignore(just(Token::RParen))
        .or_not()
        .map(Option::unwrap_or_default);
    column_name()
        .then_ignore(just(Token::Colon))
        .then(col_type())
        .then(type_args)
        .then(annotation().repeated())
        .map_with_span(
            |(((name, col_type), type_args), annotations), span| Column {
                name,
                col_type,
                type_args,
                annotations,
                span,
            },
        )
}
