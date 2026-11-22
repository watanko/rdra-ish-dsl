//! Transition bindings, literals, and dispatch construction.

use super::*;

pub(super) fn declared_types(domain: &DomainModel) -> BTreeMap<String, Declared<'_>> {
    let mut out = BTreeMap::new();
    for record in &domain.records {
        if record.kind == super::ir::RecordKind::Entity {
            out.insert(record.name.clone(), Declared::Record(record));
        }
    }
    for sum in &domain.sums {
        out.insert(sum.name.clone(), Declared::Sum(sum));
    }
    out
}

/// The functions of one action, or why it gets none.
#[allow(clippy::too_many_arguments)]
pub(super) fn functions(
    model: &SemanticModel,
    ts: &TransitionSystem,
    action: &Action,
    declared: &BTreeMap<String, Declared<'_>>,
    raises: &[String],
    steps: Option<(u8, u8)>,
    marks: &mut Vec<super::ir::MarkDecl>,
    site: &dyn Fn(&LocatedSpan) -> Option<String>,
) -> Result<Vec<FnDecl>, String> {
    let entity_key = |e: EntId| ts.entity(e).key;
    let type_of = |e: EntId| -> Result<&Declared<'_>, String> {
        let id = &model.entities[entity_key(e)].id;
        declared
            .get(&type_ident(id))
            .ok_or_else(|| format!("entity {id} has no generated type"))
    };

    // Which params are rows of the function, by role.
    for param in &action.params {
        match param.role {
            ParamRole::New
            | ParamRole::Existing
            | ParamRole::Parent { .. }
            | ParamRole::Earlier { .. } => {}
            ParamRole::Triggered => {}
        }
    }
    for guard in &action.guards {
        match guard.why {
            GuardWhy::TransitionFrom | GuardWhy::Owner => {}
            GuardWhy::Condition { later: None, .. } if !guard.reads_successor() => {}
            GuardWhy::Raised { .. } => {}
            GuardWhy::Clock => return Err("reads `now` (phase 5)".into()),
            GuardWhy::Link => return Err("links, relinks, or unlinks a row (phase 5)".into()),
            GuardWhy::Copy => return Err("copies a value from another row (phase 5)".into()),
            GuardWhy::ArmCondition { .. } => {}
            GuardWhy::Run { .. } => {}
            GuardWhy::Condition { .. } => {
                return Err(
                    "a condition on what the use case creates or a later step writes (phase 5)"
                        .into(),
                )
            }
        }
    }

    // The tag var of each param's entity, when it is a state type.
    let tag_var = |p: usize| -> Option<VarId> {
        let e = action.params[p].entity;
        let Declared::Sum(sum) = type_of(e).ok()? else {
            return None;
        };
        ts.var_ids().find(|v| {
            matches!(&ts.var(*v).kind, VarKind::Column { entity, column, .. }
                if *entity == e && *column == sum.tag.wire)
        })
    };

    // The arms each param's tag takes: (from, to) by value names.
    let mut arms: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    for effect in &action.effects {
        if let Effect::Transition {
            var,
            param,
            arms: a,
            ..
        } = effect
        {
            let p = usize::from(*param);
            let Domain::Enum(values) = &ts.var(*var).domain else {
                return Err("a transition on a column that is not an enum".into());
            };
            let named: Vec<(String, String)> = a
                .iter()
                .map(|(f, t)| {
                    (
                        values[usize::from(f.0)].clone(),
                        values[usize::from(t.0)].clone(),
                    )
                })
                .collect();
            if Some(*var) != tag_var(p) {
                // A second lifecycle column: a branch on its value, not a state type.
                let mut froms: Vec<&String> = named.iter().map(|(f, _)| f).collect();
                froms.sort();
                froms.dedup();
                if froms.len() != named.len() {
                    return Err("a choice among arms of a second lifecycle column".into());
                }
                continue;
            }
            arms.insert(p, named);
        }
    }

    // The source states of each param.
    let mut sources: Vec<Vec<Source<'_>>> = Vec::new();
    for (p, param) in action.params.iter().enumerate() {
        let declared = type_of(param.entity)?;
        let list = match (param.role, declared) {
            // A row of an earlier step: the step reads it through its run mark only.
            (ParamRole::Earlier { .. }, _) => vec![Source {
                ty: String::new(),
                fields: Vec::new(),
                always_null: Vec::new(),
                variant: None,
            }],
            // The row an event was raised on: read only, in any state, as its flat row.
            (ParamRole::Triggered, Declared::Sum(sum)) => vec![Source {
                ty: sum.row.clone(),
                fields: sum.row_fields.clone(),
                always_null: Vec::new(),
                variant: None,
            }],
            (ParamRole::New, Declared::Record(r)) => vec![record_source(r)],
            (ParamRole::New, Declared::Sum(_)) => vec![Source {
                ty: String::new(),
                fields: Vec::new(),
                always_null: Vec::new(),
                variant: None,
            }],
            (_, Declared::Record(r)) => vec![record_source(r)],
            (_, Declared::Sum(sum)) => sum
                .variants
                .iter()
                .filter(|v| match arms.get(&p) {
                    Some(a) => a.iter().any(|(from, _)| *from == v.value.wire),
                    None => true,
                })
                .map(variant_source)
                .collect(),
        };
        sources.push(list);
    }

    let combos = combinations(&sources);
    if combos.len() > MAX_COMBINATIONS {
        return Err(format!(
            "{} combinations of source states (more than {MAX_COMBINATIONS})",
            combos.len()
        ));
    }
    let varies: Vec<bool> = sources.iter().map(|s| s.len() > 1).collect();
    let uc = match &action.origin {
        ActionOrigin::UseCase { uc, .. } => *uc,
        ActionOrigin::Clock => unreachable!("only use case actions reach here"),
    };
    let usecase = &model.use_cases[uc];

    let mut out = Vec::new();
    for combo in combos {
        let picked: Vec<&Source<'_>> = combo
            .iter()
            .enumerate()
            .map(|(p, i)| &sources[p][*i])
            .collect();
        let mut base = type_ident(&action.name);
        let suffix: Vec<String> = picked
            .iter()
            .zip(&varies)
            .filter(|(_, v)| **v)
            .filter_map(|(s, _)| s.variant.map(|v| v.value.ident.clone()))
            .collect();
        if !suffix.is_empty() {
            base = format!("{base}From{}", suffix.join("And"));
        }
        // The arms each param can take from its source: several from one state are a
        // choice, and each choice is a function of its own.
        let mut options: Vec<(usize, Vec<(String, String)>)> = Vec::new();
        for (p, a) in &arms {
            let from = picked[*p].variant.map(|v| v.value.wire.clone());
            let open: Vec<(String, String)> = match &from {
                Some(from) => a.iter().filter(|(f, _)| f == from).cloned().collect(),
                None => a.clone(),
            };
            options.push((*p, open));
        }
        let mut choices: Vec<BTreeMap<usize, Vec<(String, String)>>> = vec![BTreeMap::new()];
        for (p, open) in &options {
            let mut next = Vec::new();
            for chosen in &choices {
                if open.len() > 1 && picked[*p].variant.is_some() {
                    for arm in open {
                        let mut c = chosen.clone();
                        c.insert(*p, vec![arm.clone()]);
                        next.push(c);
                    }
                } else {
                    let mut c = chosen.clone();
                    c.insert(*p, arms[p].clone());
                    next.push(c);
                }
            }
            choices = next;
        }
        if choices.len() > MAX_COMBINATIONS {
            return Err(format!(
                "{} choices among arms (more than {MAX_COMBINATIONS})",
                choices.len()
            ));
        }
        for chosen in choices {
            let mut name = base.clone();
            let to: Vec<String> = options
                .iter()
                .filter(|(p, open)| open.len() > 1 && picked[*p].variant.is_some())
                .filter_map(|(p, _)| chosen[p].first().map(|(_, t)| type_ident(t)))
                .collect();
            if !to.is_empty() {
                name = format!("{name}To{}", to.join("And"));
            }
            match function(
                model,
                ts,
                action,
                &picked,
                &chosen,
                declared,
                raises,
                site,
                &name,
                &usecase.id,
                steps,
                marks,
            ) {
                Ok(mut f) => {
                    f.doc
                        .insert(0, format!("{} ({}).", usecase.label, usecase.id));
                    for (p, open) in &options {
                        if open.len() < 2 || picked[*p].variant.is_none() {
                            continue;
                        }
                        let Some(r) = f.rows.iter().position(|row| row.param == *p) else {
                            continue;
                        };
                        let Some(Declared::Sum(sum)) = declared.get(&type_ident(&f.rows[r].entity))
                        else {
                            continue;
                        };
                        let FieldType::Enum(tag_enum) = &sum.tag.ty else {
                            continue;
                        };
                        let target = &chosen[p][0].1;
                        if let Some(v) = sum.variants.iter().find(|v| v.value.wire == *target) {
                            f.chosen.push((r, tag_enum.clone(), v.value.clone()));
                        }
                    }
                    out.push(f);
                }
                Err(Skip::Combination) => {}
                Err(Skip::Action(reason)) => return Err(reason),
            }
        }
    }
    Ok(out)
}

pub(super) fn record_source(record: &RecordDecl) -> Source<'_> {
    Source {
        ty: record.name.clone(),
        fields: record.fields.clone(),
        always_null: Vec::new(),
        variant: None,
    }
}

pub(super) fn variant_source(variant: &VariantDecl) -> Source<'_> {
    Source {
        ty: variant.name.clone(),
        fields: variant.fields.clone(),
        always_null: variant.always_null.iter().map(|f| f.wire.clone()).collect(),
        variant: Some(variant),
    }
}

/// Every pick of one source per param.
pub(super) fn combinations(sources: &[Vec<Source<'_>>]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for list in sources {
        let mut next = Vec::new();
        for prefix in &out {
            for i in 0..list.len() {
                let mut c = prefix.clone();
                c.push(i);
                next.push(c);
            }
        }
        out = next;
    }
    out
}

/// The write of a literal value of `domain`.
pub(super) fn literal(domain: &Domain, value: Value, ts: &TransitionSystem) -> Option<Write> {
    Some(match domain {
        Domain::Enum(values) => {
            let wire = values.get(usize::from(value.0))?.clone();
            Write::Lit(Lit::Enum {
                ty: String::new(),
                value: EnumValue {
                    ident: type_ident(&wire),
                    wire,
                    label: None,
                },
            })
        }
        Domain::Bool => Write::Lit(Lit::Bool(value == Value::TRUE)),
        Domain::Nullable if value == Value::NULL => Write::Null,
        Domain::Nullable => Write::Input,
        Domain::Int | Domain::NullableInt => match domain.number(value) {
            Some(n) => Write::Lit(Lit::Int(n)),
            None => Write::Null,
        },
        Domain::Existence | Domain::Instance { .. } => {
            let _ = ts;
            return None;
        }
    })
}

/// The start value of a created row's column, by its var: `Some(Some(lit))`, `Some(None)` for
/// null, `None` when no var holds the column (the caller passes it).
pub(super) fn start_literal(
    ts: &TransitionSystem,
    e: EntId,
    column: &str,
    target: &Field,
) -> Option<Option<Lit>> {
    let var = ts.var_ids().find(|v| {
        matches!(&ts.var(*v).kind, VarKind::Column { entity, column: c, .. } if *entity == e && c == column)
    })?;
    let decl = ts.var(var);
    match literal(&decl.domain, decl.base, ts)? {
        Write::Lit(Lit::Enum { value, .. }) => {
            let FieldType::Enum(ty) = &target.ty else {
                return None;
            };
            Some(Some(Lit::Enum {
                ty: ty.clone(),
                value,
            }))
        }
        Write::Lit(lit) => Some(Some(lit)),
        Write::Null => Some(None),
        Write::Input | Write::Parent(..) | Write::Switch(_) => None,
    }
}

/// The start value of a created row's tag.
pub(super) fn start_value(ts: &TransitionSystem, e: EntId, tag: &str) -> Option<String> {
    let var = ts.var_ids().find(|v| {
        matches!(&ts.var(*v).kind, VarKind::Column { entity, column, .. } if *entity == e && column == tag)
    })?;
    let decl = ts.var(var);
    match &decl.domain {
        Domain::Enum(values) => values.get(usize::from(decl.base.0)).cloned(),
        _ => None,
    }
}

/// The id type and id field of an entity's type, when it has one.
pub(super) fn declared_id(
    declared: &BTreeMap<String, Declared<'_>>,
    entity: &str,
    fields: &[Field],
) -> Option<(String, Field)> {
    let _ = declared.get(&type_ident(entity))?;
    fields.iter().find_map(|f| match &f.ty {
        FieldType::Id(ty) if *ty == format!("{}Id", type_ident(entity)) => {
            Some((ty.clone(), f.clone()))
        }
        _ => None,
    })
}

/// A literal typed for its target field: an enum value takes the field's enum type.
pub(super) fn typed(lit: &Lit, target: &Field) -> Option<Lit> {
    match (lit, &target.ty) {
        (Lit::Enum { value, .. }, FieldType::Enum(ty)) => Some(Lit::Enum {
            ty: ty.clone(),
            value: value.clone(),
        }),
        (Lit::Enum { .. }, _) => None,
        (other, _) => Some(other.clone()),
    }
}

/// The input named `name`, added when the function has none by that name yet.
pub(super) fn input_index(
    inputs: &mut Vec<FnInput>,
    name: String,
    ty: FieldType,
    doc: String,
) -> usize {
    if let Some(i) = inputs.iter().position(|x| x.name == name) {
        return i;
    }
    inputs.push(FnInput {
        name,
        ty,
        nullable: false,
        doc,
    });
    inputs.len() - 1
}

/// One dispatcher per action whose functions take a state type or are several.
pub(super) fn dispatchers(fns: &[FnDecl], domain: &DomainModel) -> Vec<DispatchDecl> {
    let mut actions: Vec<&str> = Vec::new();
    for f in fns {
        if !actions.contains(&f.action.as_str()) {
            actions.push(&f.action);
        }
    }
    let sum_of = |entity: &str| domain.sums.iter().find(|s| s.entity == entity);
    let mut out = Vec::new();
    for action in actions {
        let group: Vec<&FnDecl> = fns.iter().filter(|f| f.action == action).collect();
        let first = group[0];
        let any_state = first.rows.iter().any(|r| r.state.is_some());
        if group.len() == 1 && !any_state {
            continue;
        }
        let rows: Vec<(String, String)> = first
            .rows
            .iter()
            .map(|r| {
                let ty = match sum_of(&r.entity) {
                    Some(sum) if r.state.is_some() => sum.name.clone(),
                    _ => r.ty.clone(),
                };
                (r.name.clone(), ty)
            })
            .collect();
        let mut inputs: Vec<FnInput> = Vec::new();
        let mut choices: Vec<(String, String)> = Vec::new();
        let mut cases = Vec::new();
        let pascal = type_ident(action);
        for f in &group {
            let mut taken = Vec::new();
            for input in &f.inputs {
                let i = match inputs.iter().position(|x| x.name == input.name) {
                    Some(i) => i,
                    None => {
                        inputs.push(input.clone());
                        inputs.len() - 1
                    }
                };
                taken.push(i);
            }
            let states = f
                .rows
                .iter()
                .map(|r| {
                    let (tag, value) = r.state.as_ref()?;
                    let sum = sum_of(&r.entity)?;
                    let v = sum.variants.iter().find(|v| v.value.wire == *value)?;
                    Some((r.ty.clone(), tag.clone(), v.value.clone()))
                })
                .collect();
            let mut choice = Vec::new();
            for (r, tag_enum, target) in &f.chosen {
                let name = format!("{}_to", f.rows[*r].name);
                let i = match choices.iter().position(|(n, _)| *n == name) {
                    Some(i) => i,
                    None => {
                        choices.push((name, tag_enum.clone()));
                        choices.len() - 1
                    }
                };
                choice.push((i, target.clone()));
            }
            let case = f.name.strip_prefix(&pascal).unwrap_or(&f.name);
            cases.push(DispatchCase {
                function: f.name.clone(),
                result: f.result.clone(),
                case: if case.is_empty() {
                    "Ran".to_string()
                } else {
                    case.to_string()
                },
                rejects: !f.conditions.is_empty(),
                states,
                inputs: taken,
                choice,
            });
        }
        // The arguments after the rows: the run marks, then the event.
        let mut marks: Vec<(String, String)> = first
            .marks_in
            .iter()
            .map(|m| (m.name.clone(), m.ty.clone()))
            .collect();
        if let Some(ev) = &first.event_in {
            marks.push((ev.name.clone(), ev.ty.clone()));
        }
        out.push(DispatchDecl {
            marks,
            choices,
            name: format!("Run{pascal}"),
            usecase: first.usecase.clone(),
            outcome: format!("{pascal}Outcome"),
            rows,
            inputs,
            rejects: cases.iter().any(|c| c.rejects),
            cases,
        });
    }
    out
}

/// The payload of an event: the id of each row its raise marks, by the system's `Raised`
/// vars. An event no use case raise orders anything by has none.
pub(super) fn event_fields(
    model: &SemanticModel,
    ts: &TransitionSystem,
    declared: &BTreeMap<String, Declared<'_>>,
    event: &str,
) -> Vec<super::ir::EventField> {
    let _ = model;
    let mut out: Vec<super::ir::EventField> = Vec::new();
    for v in ts.var_ids() {
        let VarKind::Raised { entity, event: ev } = &ts.var(v).kind else {
            continue;
        };
        if ev != event {
            continue;
        }
        let id = ts.entity(*entity).id.clone();
        let fields = match declared.get(&type_ident(&id)) {
            Some(Declared::Record(r)) => &r.fields,
            Some(Declared::Sum(s)) => &s.row_fields,
            None => continue,
        };
        let want = format!("{}Id", type_ident(&id));
        if !fields.iter().any(|f| f.ty == FieldType::Id(want.clone())) {
            continue;
        }
        let field = super::ir::EventField {
            name: format!("{}_id", snake(&id)),
            entity: id,
            id_ty: want,
        };
        if !out.contains(&field) {
            out.push(field);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `pred` with each read of a param's successor tag replaced by whether the function's target
/// is that value; `None` when it reads some other var of a successor.
pub(super) fn fold_next(pred: &Pred, targets: &BTreeMap<usize, (VarId, Value)>) -> Option<Pred> {
    let rec = |p: &Pred| fold_next(p, targets);
    // The target a read of a successor's tag stands for; `None` for any other read of a
    // successor, which this phase does not fold.
    let next_tag = |t: &Term| -> Option<Value> {
        let Term::Slot {
            var,
            at: At::Next(p),
        } = t
        else {
            return None;
        };
        let (tag, to) = targets.get(&usize::from(*p))?;
        (var == tag).then_some(*to)
    };
    let reads_next = |t: &Term| {
        matches!(
            t,
            Term::Slot {
                at: At::Next(_),
                ..
            } | Term::Index(At::Next(_))
        )
    };
    let truth = |b: bool| if b { Pred::True } else { Pred::False };
    Some(match pred {
        Pred::Cmp(a, op, b) if reads_next(a) || reads_next(b) => {
            let (Some(to), Term::Lit(v), TsCmp::Eq) = (next_tag(a), b, op) else {
                return None;
            };
            truth(to == *v)
        }
        Pred::In(a, vs) if reads_next(a) => truth(vs.contains(&Term::Lit(next_tag(a)?))),
        Pred::Not(p) => Pred::Not(Box::new(rec(p)?)),
        Pred::And(ps) => Pred::And(ps.iter().map(rec).collect::<Option<_>>()?),
        Pred::Or(ps) => Pred::Or(ps.iter().map(rec).collect::<Option<_>>()?),
        Pred::Forall { entity, body } => Pred::Forall {
            entity: *entity,
            body: Box::new(rec(body)?),
        },
        Pred::Exists { entity, body } => Pred::Exists {
            entity: *entity,
            body: Box::new(rec(body)?),
        },
        other => other.clone(),
    })
}

/// The id and label of the `.when` conditions of the arms the function takes.
pub(super) fn arm_conditions(
    model: &SemanticModel,
    ts: &TransitionSystem,
    action: &Action,
    chosen: &BTreeMap<usize, Vec<(String, String)>>,
) -> (String, String) {
    let mut ids: Vec<String> = Vec::new();
    for effect in &action.effects {
        let Effect::Transition {
            var,
            param,
            arms,
            conds,
            ..
        } = effect
        else {
            continue;
        };
        let Some(taken) = chosen.get(&usize::from(*param)) else {
            continue;
        };
        let Domain::Enum(values) = &ts.var(*var).domain else {
            continue;
        };
        let name = |v: Value| values[usize::from(v.0)].clone();
        for ((f, t), arm) in arms.iter().zip(conds) {
            if !taken.contains(&(name(*f), name(*t))) {
                continue;
            }
            for (_, id) in arm {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    let labels: Vec<String> = ids
        .iter()
        .filter_map(|id| {
            model
                .conditions
                .values()
                .find(|c| &c.id == id)
                .map(|c| c.label.clone())
        })
        .collect();
    (ids.join(" or "), labels.join(" / "))
}

/// Whether every slot `pred` reads is a param the action creates.
pub(super) fn reads_only_created(pred: &Pred, action: &Action) -> bool {
    let created = |t: &Term| match t {
        Term::Slot {
            at: At::Param(p), ..
        }
        | Term::Index(At::Param(p)) => action.params[usize::from(*p)].role == ParamRole::New,
        Term::Slot { .. } | Term::Index(_) => false,
        _ => true,
    };
    match pred {
        Pred::True | Pred::False => true,
        Pred::Cmp(a, _, b) => created(a) && created(b),
        Pred::In(a, vs) => created(a) && vs.iter().all(created),
        Pred::Not(p) => reads_only_created(p, action),
        Pred::And(ps) | Pred::Or(ps) => ps.iter().all(|p| reads_only_created(p, action)),
        Pred::Forall { .. } | Pred::Exists { .. } => false,
    }
}

/// Points each quantifier at the input its list became.
pub(super) fn remap_lists(e: &mut Expr, to: &[usize]) {
    match e {
        Expr::Not(x) => remap_lists(x, to),
        Expr::And(xs) | Expr::Or(xs) => xs.iter_mut().for_each(|x| remap_lists(x, to)),
        Expr::Exists { input, body, .. } | Expr::Forall { input, body, .. } => {
            *input = to[*input];
            remap_lists(body, to);
        }
        _ => {}
    }
}
