//! Declared business flow structure, without inferring order from labels or use cases.

use rdra_ish_core::model::{NodeRef, RelKind};
use rdra_ish_core::SemanticModel;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize)]
pub(super) struct BusinessFlow {
    id: String,
    label: String,
    bucs: Vec<String>,
    steps: Vec<BusinessStep>,
    links: Vec<BusinessLink>,
}

#[derive(Serialize)]
struct BusinessStep {
    id: String,
    label: String,
    covers: Vec<Target>,
    performers: Vec<String>,
}

#[derive(Serialize)]
struct Target {
    kind: String,
    id: String,
}

#[derive(Serialize)]
struct BusinessLink {
    kind: String,
    from: String,
    to: String,
}

pub(super) fn build(model: &SemanticModel) -> Vec<BusinessFlow> {
    let mut flows: Vec<_> = model.flows.iter().collect();
    flows.sort_by_key(|(_, flow)| &flow.id);
    flows
        .into_iter()
        .map(|(key, flow)| {
            let node = NodeRef::Flow(key);
            let mut bucs: Vec<_> = model
                .relations
                .iter()
                .filter_map(|rel| {
                    if rel.kind == RelKind::Contains && rel.to == node {
                        if let NodeRef::Buc(buc) = rel.from {
                            return Some(model.bucs[buc].id.clone());
                        }
                    }
                    None
                })
                .collect();
            bucs.sort();
            let steps: HashSet<_> = model
                .relations
                .iter()
                .filter(|rel| {
                    rel.kind == RelKind::Contains
                        && rel.from == node
                        && matches!(rel.to, NodeRef::Step(_))
                })
                .map(|rel| rel.to.clone())
                .collect();
            let mut declarations: Vec<_> = steps
                .iter()
                .map(|step| {
                    let mut covers: Vec<_> = model
                        .relations
                        .iter()
                        .filter(|rel| rel.kind == RelKind::Covers && &rel.from == step)
                        .map(|rel| Target {
                            kind: rel.to.kind_tag().into(),
                            id: model
                                .node_id(&rel.to)
                                .expect("resolved cover target")
                                .into(),
                        })
                        .collect();
                    covers.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
                    let mut performers: Vec<_> = model
                        .relations
                        .iter()
                        .filter(|rel| rel.kind == RelKind::Performs && &rel.to == step)
                        .map(|rel| {
                            model
                                .node_id(&rel.from)
                                .expect("resolved performer")
                                .to_string()
                        })
                        .collect();
                    performers.sort();
                    BusinessStep {
                        id: model.node_id(step).expect("resolved step").into(),
                        label: model.node_label(step).expect("resolved step label").into(),
                        covers,
                        performers,
                    }
                })
                .collect();
            declarations.sort_by(|a, b| a.id.cmp(&b.id));
            let mut links: Vec<_> = model
                .relations
                .iter()
                .filter_map(|rel| {
                    if !steps.contains(&rel.from) || !matches!(rel.to, NodeRef::Step(_)) {
                        return None;
                    }
                    let kind = match rel.kind {
                        RelKind::Precedes => "precedes",
                        RelKind::Branches => "branches",
                        RelKind::Excepts => "excepts",
                        RelKind::Repeats => "repeats",
                        _ => return None,
                    };
                    Some(BusinessLink {
                        kind: kind.into(),
                        from: model
                            .node_id(&rel.from)
                            .expect("resolved flow source")
                            .into(),
                        to: model.node_id(&rel.to).expect("resolved flow target").into(),
                    })
                })
                .collect();
            links.sort_by(|a, b| (&a.kind, &a.from, &a.to).cmp(&(&b.kind, &b.from, &b.to)));
            BusinessFlow {
                id: flow.id.clone(),
                label: flow.label.clone(),
                bucs,
                steps: declarations,
                links,
            }
        })
        .collect()
}
