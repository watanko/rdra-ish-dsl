//! Transition functions from the transition system's actions (docs/design/algebraic-codegen.md,
//! "Transition functions"). The functions read the one system `states`, `check`, and `verify`
//! read, so they do what the verified model does.
//!
//! An action gets one function per combination of the source states of its rows, so every
//! type in a function is concrete. An action that uses a part of the vocabulary this phase
//! does not print yet is listed as skipped, with the reason, and gets no function: the output
//! stays buildable and says what it leaves out.

mod bodies;
mod builders;
mod conditions;

use bodies::*;
use builders::*;
use conditions::*;

use std::collections::BTreeMap;

use rdra_ish_core::collect_event_flows;
use rdra_ish_core::lifecycle::{
    Action, ActionOrigin, At, CmpOp as TsCmp, Domain, Effect, EntId, GuardWhy, ParamRole, Pred,
    Rhs, Term, TransitionSystem, Value, VarId, VarKind,
};
use rdra_ish_core::location::LocatedSpan;
use rdra_ish_core::model::Raiser;
use rdra_ish_core::SemanticModel;

use super::ir;
use super::ir::{
    Adapt, ArgCheck, Assigned, CmpOp, CondGuard, DispatchCase, DispatchDecl, DomainModel,
    EnumValue, EventDecl, Expr, Field, FieldType, FnDecl, FnInput, Lit, OutKind, RecordDecl,
    RowOut, RowParam, Skipped, SumDecl, VariantDecl,
};
use super::{type_ident, CodegenError, Names};

/// The most functions one action gets; past it the action is skipped.
const MAX_COMBINATIONS: usize = 16;

/// The type of an entity, as `build` declared it.
enum Declared<'d> {
    Record(&'d RecordDecl),
    Sum(&'d SumDecl),
}

/// One source state of a row: the type, its fields, and (for a state type) its variant.
#[derive(Clone)]
struct Source<'d> {
    ty: String,
    fields: Vec<Field>,
    always_null: Vec<String>,
    variant: Option<&'d VariantDecl>,
}

/// Why an action or one of its combinations gets no function.
enum Skip {
    /// The whole action: the vocabulary is not printed yet.
    Action(String),
    /// This combination: the verified model never runs the action from it.
    Combination,
}

pub(super) fn add_functions(
    model: &SemanticModel,
    ts: &TransitionSystem,
    domain: &mut DomainModel,
    names: &mut Names,
    site: &dyn Fn(&LocatedSpan) -> Option<String>,
) -> Result<(), CodegenError> {
    let flows = collect_event_flows(model);
    let mut events: BTreeMap<String, EventDecl> = BTreeMap::new();
    let mut fns = Vec::new();
    let mut skipped = Vec::new();
    let mut marks: Vec<super::ir::MarkDecl> = Vec::new();
    // The number of steps of each split use case.
    let mut steps_of: BTreeMap<String, u8> = BTreeMap::new();
    for action in ts.actions() {
        if let ActionOrigin::UseCase {
            uc, step: Some(i), ..
        } = &action.origin
        {
            let n = steps_of.entry(model.use_cases[*uc].id.clone()).or_insert(0);
            *n = (*n).max(*i);
        }
    }
    {
        let declared = declared_types(domain);
        for action in ts.actions() {
            let ActionOrigin::UseCase {
                uc, step, trigger, ..
            } = &action.origin
            else {
                continue;
            };
            let steps = step.map(|i| (i, steps_of[&model.use_cases[*uc].id]));
            // A split use case raises its events at its last step.
            let raises: Vec<String> = flows
                .iter()
                .filter(|f| f.raised_by.contains(&Raiser::UseCase(*uc)))
                .filter(|_| steps.is_none_or(|(i, k)| i == k))
                .map(|f| model.events[f.event].id.clone())
                .collect();
            let _ = trigger;
            let reason: Option<String> = None;
            let built = match reason {
                Some(reason) => Err(reason),
                None => functions(
                    model, ts, action, &declared, &raises, steps, &mut marks, site,
                ),
            };
            match built {
                Ok(list) if list.is_empty() => skipped.push(Skipped {
                    action: action.name.clone(),
                    reason: "no source state the verified model runs it from".to_string(),
                }),
                Ok(list) => {
                    for event in &raises {
                        let fields = event_fields(model, ts, &declared, event);
                        events.entry(event.clone()).or_insert_with(|| EventDecl {
                            name: type_ident(event),
                            doc: model
                                .events
                                .values()
                                .find(|e| &e.id == event)
                                .map(|e| vec![e.label.clone()])
                                .unwrap_or_default(),
                            fields,
                        });
                    }
                    for f in &list {
                        if let Some(ev) = &f.event_in {
                            let id = model
                                .events
                                .values()
                                .find(|e| type_ident(&e.id) == ev.ty)
                                .map(|e| e.id.clone())
                                .unwrap_or_default();
                            let fields = event_fields(model, ts, &declared, &id);
                            events.entry(id.clone()).or_insert_with(|| EventDecl {
                                name: ev.ty.clone(),
                                doc: model
                                    .events
                                    .values()
                                    .find(|e| e.id == id)
                                    .map(|e| vec![e.label.clone()])
                                    .unwrap_or_default(),
                                fields,
                            });
                        }
                    }
                    fns.extend(list);
                }
                Err(reason) => skipped.push(Skipped {
                    action: action.name.clone(),
                    reason,
                }),
            }
        }
    }
    for event in events.values() {
        names.claim(event.name.clone(), format!("the event {}", event.name))?;
    }
    for f in &fns {
        names.claim(f.name.clone(), format!("the function of {}", f.usecase))?;
        names.claim(f.result.clone(), format!("the result of {}", f.name))?;
    }
    let dispatchers = dispatchers(&fns, domain);
    for d in &dispatchers {
        names.claim(d.name.clone(), format!("the dispatcher of {}", d.usecase))?;
        names.claim(d.outcome.clone(), format!("the outcome of {}", d.name))?;
    }
    domain.dispatchers = dispatchers;
    // The run marks, and what a split use case asks of the code that stores them.
    marks.sort_by(|a, b| a.name.cmp(&b.name));
    marks.dedup();
    let mut split: Vec<String> = marks.iter().map(|m| m.usecase.clone()).collect();
    split.sort();
    split.dedup();
    for uc in &split {
        let names: Vec<&str> = marks
            .iter()
            .filter(|m| &m.usecase == uc)
            .map(|m| m.name.as_str())
            .collect();
        let id = format!("O{}", domain.obligations.len() + 1);
        domain.obligations.push(super::ir::Obligation {
            id,
            invariant: None,
            text: format!(
                "{uc} runs as separate transactions linked by run marks ({}): store each mark with its row in the transaction of the step that returns it; keep at most one mark of {uc} per row and start step 1 only on a row with none; never abandon a run, but retry a step that returns NotEnabled or Rejected until it runs.",
                names.join(", ")
            ),
            site: None,
        });
    }
    // A list a `.has` / `.none` ranges over must hold every present row of its entity.
    let mut lists: Vec<String> = fns
        .iter()
        .flat_map(|f| f.inputs.iter())
        .filter_map(|i| match &i.ty {
            FieldType::Rows(ty) => Some(ty.clone()),
            _ => None,
        })
        .collect();
    lists.sort();
    lists.dedup();
    for ty in lists {
        let id = format!("O{}", domain.obligations.len() + 1);
        domain.obligations.push(super::ir::Obligation {
            id,
            invariant: None,
            text: format!(
                "a function input of {ty} rows holds every present row of the entity, read in the function's transaction: a `.has` / `.none` condition reads them all."
            ),
            site: None,
        });
    }
    for m in &marks {
        names.claim(
            m.name.clone(),
            format!("the run mark of {} at step {}", m.usecase, m.step),
        )?;
    }
    domain.marks = marks;
    domain.events = events.into_values().collect();
    domain.fns = fns;
    domain.skipped = skipped;
    Ok(())
}

/// What an effect writes to a column.
#[derive(Clone, Debug)]
enum Write {
    Lit(Lit),
    Null,
    /// A second lifecycle column: `(from, to)` per arm.
    Switch(Vec<(String, String)>),
    /// A value the model leaves open: the caller passes it.
    Input,
    /// The key of the parent param, by its key column.
    Parent(usize, String),
}

/// Where a slot reference of a predicate points: a row, and its state when it has one.
pub(super) type Bind<'a, 'd> = &'a dyn Fn(At) -> Option<(usize, Option<&'d VariantDecl>)>;

/// The flat row type and fields of an entity, by its id.
pub(super) type RowTypeOf<'a> = &'a dyn Fn(&str) -> Option<(String, Vec<Field>)>;

/// What a quantifier (`.has` / `.none`) needs: the rows it binds and the lists they come from.
pub(super) struct Quant<'a> {
    pub model: &'a SemanticModel,
    /// The flat row type and fields of an entity.
    pub row_type: RowTypeOf<'a>,
    /// The `Bound` index of the first quantifier's row.
    pub base: usize,
    /// The function's rows: a quantifier's row is numbered after them.
    pub n_rows: usize,
    pub elems: Vec<RowParam>,
    /// The row each enclosing quantifier binds, outermost first.
    pub stack: Vec<usize>,
    /// `(input name, row type)` per list a quantifier ranges over.
    pub lists: Vec<(String, String)>,
}

pub(super) fn expr(
    ts: &TransitionSystem,
    pred: &Pred,
    rows: &[RowParam],
    bind: Bind<'_, '_>,
    quant: &mut Option<&mut Quant<'_>>,
) -> Result<Expr, String> {
    Ok(match pred {
        Pred::True => Expr::True,
        Pred::False => Expr::False,
        Pred::Not(p) => Expr::Not(Box::new(expr(ts, p, rows, bind, quant)?)),
        Pred::And(ps) => {
            let mut out = Vec::new();
            for p in ps {
                out.push(expr(ts, p, rows, bind, quant)?);
            }
            Expr::And(out)
        }
        Pred::Or(ps) => {
            let mut out = Vec::new();
            for p in ps {
                out.push(expr(ts, p, rows, bind, quant)?);
            }
            Expr::Or(out)
        }
        Pred::In(term, values) => {
            let mut out = Vec::new();
            for v in values {
                out.push(atom(ts, term, TsCmp::Eq, v, rows, bind, quant)?);
            }
            Expr::Or(out)
        }
        Pred::Cmp(a, op, b) => atom(ts, a, *op, b, rows, bind, quant)?,
        Pred::Forall { entity, body } | Pred::Exists { entity, body } => {
            let Some(q) = quant.as_deref_mut() else {
                return Err("a condition over related rows, `.has` or `.none` (phase 5)".into());
            };
            let id = ts.entity(*entity).id.clone();
            let (ty, fields) = (q.row_type)(&id)
                .ok_or_else(|| format!("related rows of {id}, which has no generated type"))?;
            let list = format!("{}_rows", super::actions::snake(&id));
            let input = match q.lists.iter().position(|(n, _)| *n == list) {
                Some(i) => i,
                None => {
                    q.lists.push((list, ty.clone()));
                    q.lists.len() - 1
                }
            };
            let k = q.elems.len();
            q.elems.push(RowParam {
                name: format!("{}_{k}", super::actions::snake(&id)),
                param: usize::MAX,
                state: None,
                entity: id,
                ty,
                fields,
                always_null: Vec::new(),
            });
            let elem = q.n_rows + k;
            q.stack.push(elem);
            let inner = expr(ts, body, rows, bind, quant);
            if let Some(q) = quant.as_deref_mut() {
                q.stack.pop();
            }
            let body = Box::new(inner?);
            match pred {
                Pred::Exists { .. } => Expr::Exists { input, elem, body },
                _ => Expr::Forall { input, elem, body },
            }
        }
    })
}

/// snake_case of an id: `CustomerAccount` is `customer_account`.
pub(super) fn snake(id: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in id.chars() {
        if ch == '-' || ch == ' ' {
            out.push('_');
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
            prev_lower = false;
        } else {
            out.push(ch);
            prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        }
    }
    out
}

/// Folds the constant parts of an expression.
pub(super) fn simplify(e: Expr) -> Expr {
    match e {
        Expr::Exists { input, elem, body } => match simplify(*body) {
            Expr::False => Expr::False,
            body => Expr::Exists {
                input,
                elem,
                body: Box::new(body),
            },
        },
        Expr::Forall { input, elem, body } => match simplify(*body) {
            Expr::True => Expr::True,
            body => Expr::Forall {
                input,
                elem,
                body: Box::new(body),
            },
        },
        Expr::Not(inner) => match simplify(*inner) {
            Expr::True => Expr::False,
            Expr::False => Expr::True,
            Expr::Not(x) => *x,
            other => Expr::Not(Box::new(other)),
        },
        Expr::And(items) => {
            let mut kept = Vec::new();
            for item in items.into_iter().map(simplify) {
                match item {
                    Expr::True => {}
                    Expr::False => return Expr::False,
                    other => kept.push(other),
                }
            }
            match kept.len() {
                0 => Expr::True,
                1 => kept.pop().expect("one item"),
                _ => Expr::And(kept),
            }
        }
        Expr::Or(items) => {
            let mut kept = Vec::new();
            for item in items.into_iter().map(simplify) {
                match item {
                    Expr::False => {}
                    Expr::True => return Expr::True,
                    other => kept.push(other),
                }
            }
            match kept.len() {
                0 => Expr::False,
                1 => kept.pop().expect("one item"),
                _ => Expr::Or(kept),
            }
        }
        other => other,
    }
}
