//! Event receipts and run marks produced by an action frame.

use super::*;

impl Frame<'_> {
    /// A row the event was raised on is the event's row.
    pub(super) fn raised_on_checks(&self, checks: &mut Vec<ArgCheck>) -> Result<(), Skip> {
        for (p, event_field) in &self.raised_on {
            if let Some(&r) = self.row_of.get(p) {
                let (_, id_field) = self.cx.id_of(&self.rows[r].entity).ok_or_else(|| {
                    unsupported("an event raised on an entity with no one-column key")
                })?;
                checks.push(ArgCheck::EventRow {
                    row: r,
                    field: id_field,
                    event_field: event_field.clone(),
                    message: format!(
                        "rdra-ish: the {} row is not the row the event was raised on",
                        self.rows[r].entity
                    ),
                });
            }
        }
        Ok(())
    }

    /// The events the use case raises, with the ids of the rows each raise marks.
    pub(super) fn raised(
        &self,
        raised_at: &[(String, usize)],
        inputs: &[FnInput],
    ) -> Result<Vec<RaisedEvent>, Skip> {
        let cx = self.cx;
        let mut raised: Vec<RaisedEvent> = Vec::new();
        for event in cx.raises {
            let mut ids = Vec::new();
            for field in event_fields(cx.ts, cx.declared, event) {
                let at = raised_at
                    .iter()
                    .find(|(e, p)| e == event && cx.entity_id(*p) == field.entity)
                    .map(|(_, p)| *p);
                let Some(p) = at else {
                    return Err(unsupported("raises an event without marking its row"));
                };
                let (_, id_field) = cx.id_of(&field.entity).ok_or_else(|| {
                    unsupported("an event raised on an entity with no one-column key")
                })?;
                let source = self.mark_source(
                    p,
                    &id_field,
                    inputs,
                    "an event raised on a created row with no id input",
                )?;
                ids.push((field, source));
            }
            raised.push(RaisedEvent {
                ty: type_ident(event),
                ids,
            });
        }
        Ok(raised)
    }

    /// The marks the step leaves for the next one.
    pub(super) fn marks_out(
        &self,
        runs: &[(usize, u8)],
        inputs: &[FnInput],
        marks: &mut Vec<MarkDecl>,
    ) -> Result<Vec<MarkOut>, Skip> {
        let cx = self.cx;
        let mut marks_out: Vec<MarkOut> = Vec::new();
        for &(p, step) in runs {
            if step == 0 {
                continue; // the run ends on this row: its mark is consumed
            }
            let entity = cx.entity_id(p);
            let (id_ty, id_field) = cx
                .id_of(&entity)
                .ok_or_else(|| unsupported("a run mark on an entity with no one-column key"))?;
            let source = self.mark_source(
                p,
                &id_field,
                inputs,
                "a run mark on a created row with no id input",
            )?;
            let ty = cx.declare_mark(marks, &entity, step, id_ty);
            marks_out.push(MarkOut {
                name: format!("{}_mark", snake(&entity)),
                ty,
                param: p,
                step,
                source,
            });
        }
        Ok(marks_out)
    }
}
