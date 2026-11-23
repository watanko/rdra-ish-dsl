//! Birth-only data never affects a later step. Keep a concrete representative of each
//! state class that agrees outside that data, retaining existence, owners and slot bounds.
//! Every creation choice and post-guard is still evaluated before states are interned.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ops::Range;

use crate::lifecycle::expr::State;
use crate::lifecycle::reduce::Step;
use crate::lifecycle::types::{At, Effect, EntId, ParamRole, Term, VarId, VarKind};
use crate::lifecycle::TransitionSystem;

#[derive(Clone, Default)]
pub(in crate::lifecycle::explore) struct BirthHistory {
    vars: Vec<VarId>,
    spans: Vec<Range<usize>>,
}

impl BirthHistory {
    /// Forget only columns of immutable rows, never state-model values or anything a check
    /// observes. A guard may read such a column only on the successor of its own creation.
    pub(in crate::lifecycle::explore) fn of(ts: &TransitionSystem) -> Self {
        let mut protected = BTreeSet::new();
        let mut mutable = BTreeSet::<EntId>::new();
        for inv in ts.invariants() {
            ts.pred_reads(&inv.body, &mut protected);
        }
        for after in ts.after_asserts() {
            for step in &after.steps {
                ts.pred_reads(&step.post, &mut protected);
            }
        }
        for property in ts.properties() {
            for pred in property.formula.preds() {
                ts.pred_reads(pred, &mut protected);
            }
        }
        for action in ts.actions() {
            for guard in &action.guards {
                for term in guard.pred.terms() {
                    if let Term::Slot { var, at } = term {
                        let is_birth = match at {
                            At::Next(p) => {
                                let param = &action.params[usize::from(*p)];
                                param.role == ParamRole::New
                                    && matches!(ts.var(*var).kind, VarKind::Column { entity, .. } if entity == param.entity)
                            }
                            At::Param(_) | At::Bound(_) => false,
                        };
                        if !is_birth {
                            protected.insert(*var);
                        }
                    }
                }
            }
            for effect in &action.effects {
                let p = match effect {
                    Effect::Create { param }
                    | Effect::Delete { param }
                    | Effect::Assign { param, .. }
                    | Effect::Transition { param, .. }
                    | Effect::Choose { param, .. } => *param,
                    Effect::Tick => continue,
                };
                let param = &action.params[usize::from(p)];
                if param.role != ParamRole::New {
                    mutable.insert(param.entity);
                }
                if let Effect::Choose { var, values, .. } = effect {
                    if values.iter().any(|value| !value.conds.is_empty()) {
                        protected.insert(*var);
                    }
                }
                if let Effect::Transition { var, .. } = effect {
                    protected.insert(*var);
                }
            }
        }
        let vars: Vec<_> = ts
            .var_ids()
            .filter(|v| match ts.var(*v).kind {
                VarKind::Column {
                    entity,
                    lifecycle: false,
                    ..
                } => !mutable.contains(&entity) && !protected.contains(v),
                _ => false,
            })
            .collect();
        let spans = vars
            .iter()
            .map(|v| ts.offset(*v)..ts.offset(*v) + ts.bounds.slots())
            .collect();
        Self { vars, spans }
    }

    pub(in crate::lifecycle::explore) fn len(&self) -> usize {
        self.vars.len()
    }

    pub(in crate::lifecycle::explore) fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// The representative remains a real reachable state; only its interning key omits
    /// history. No fabricated column value is fed to a guard or returned in a trace.
    pub(in crate::lifecycle::explore) fn key<'a>(&self, state: &'a State) -> Cow<'a, State> {
        if self.is_empty() {
            return Cow::Borrowed(state);
        }
        let mut key = state.clone();
        for span in &self.spans {
            key.0[span.clone()].fill(0);
        }
        Cow::Owned(key)
    }

    /// A quotient edge may enter a representative with different old birth data. Replay its
    /// action and binding from the actual preceding state, matching the retained columns.
    pub(in crate::lifecycle::explore) fn lift(
        &self,
        ts: &TransitionSystem,
        start: &State,
        steps: &[Step],
    ) -> Vec<Step> {
        if self.is_empty() {
            return steps.to_vec();
        }
        let mut current = start.clone();
        steps
            .iter()
            .map(|(act, params, target)| {
                let target = self.key(target);
                let next = ts
                    .moves(&current)
                    .find(|m| m.act == *act && m.params == *params && self.key(&m.next) == target)
                    .expect("birth-history equivalence preserves each labelled step")
                    .next;
                current = next.clone();
                (*act, params.clone(), next)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
