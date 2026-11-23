use super::*;

#[test]
fn every_rdra_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_rdra");
    let mut checked = 0;
    for root in roots() {
        let scopes: Vec<Option<String>> = std::iter::once(None)
            .chain(buc_ids(&root).into_iter().map(Some))
            .collect();
        for buc in scopes {
            let text = diagram(&root, "rdra", buc.as_deref(), &dir.join("out"));
            let chart = parse_flowchart("rdra", &text)
                .and_then(|chart| check_flowchart(&chart).map(|()| chart))
                .unwrap_or_else(|error| panic!("{root} {buc:?}: {error}\n{text}"));
            assert!(!chart.nodes.is_empty(), "{root} {buc:?}: no nodes\n{text}");
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} diagrams checked");
}

#[test]
fn the_contract_check_rejects_a_broken_diagram() {
    let good = "%% rdra-ish diagram=rdra scope=whole\n%% kinds: actor usecase\nflowchart LR\n  actor__A([\"A\"]):::actor\n  usecase__U(\"U\"):::usecase\n  actor__A -.-> usecase__U\n  classDef actor stroke:#333;\n  classDef usecase stroke:#333;\n";
    assert!(check_flowchart(&parse_flowchart("rdra", good).unwrap()).is_ok());
    for (broken, from, to) in [
        ("untagged", "\"):::usecase\n", "\")\n"),
        (
            "undeclared end",
            "usecase__U\n  classDef",
            "usecase__X\n  classDef",
        ),
        ("header kinds", "%% kinds: actor usecase", "%% kinds: actor"),
        ("wrong class", "\"]):::actor", "\"]):::usecase"),
        ("wrong shape", "actor__A([\"A\"])", "actor__A[\"A\"]"),
        ("no shape", "actor__A([", "actor__A\""),
        (
            "implied label",
            "actor__A -.-> usecase__U",
            "actor__A -.->|performs| usecase__U",
        ),
        (
            "unlisted label",
            "actor__A -.-> usecase__U",
            "actor__A -.->|reads| usecase__U",
        ),
        (
            "no relation between the ends",
            "actor__A -.-> usecase__U",
            "actor__A -.-> actor__A",
        ),
        ("emoji-era id", "  actor__A -.->", "  A -.->"),
    ] {
        let text = good.replace(from, to);
        assert_ne!(text, good, "{broken}");
        let result = parse_flowchart("rdra", &text).and_then(|chart| check_flowchart(&chart));
        assert!(result.is_err(), "{broken} passed:\n{text}");
    }
}

#[test]
fn every_drawn_kind_has_a_shape_and_every_unlabelled_edge_one_relation() {
    let dir = scratch("diagram_contract_shapes");
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut unlabelled: BTreeSet<(String, String)> = BTreeSet::new();
    for root in roots() {
        for diagram in FLOWCHARTS.into_iter().chain(["screen"]) {
            let text = diagram_with(&root, diagram, &[], &dir.join("out"));
            let chart = checked(diagram, &text, &format!("{root} {diagram}"));
            for (id, kind) in &chart.nodes {
                // A subgraph line names an element too; any other line is in its kind's shape.
                let subgraph = text.contains(&format!("subgraph {id}[\""));
                assert!(
                    subgraph || rdra_ish_emit::node_shape(kind).is_some(),
                    "{root} {diagram}: {id} is a {kind}, which has no shape"
                );
                kinds.insert(kind.clone());
            }
            for edge @ (from, relation, to) in &chart.unlabelled {
                let (f, t) = (&chart.nodes[from], &chart.nodes[to]);
                let relations: Vec<_> =
                    rdra_ish_emit::edge_relations(diagram_kind(diagram), f, t).collect();
                assert!(
                    matches!(relations.as_slice(), [only] if only.relation == relation
                        && only.label == rdra_ish_emit::EdgeLabel::Name),
                    "{root} {diagram}: {edge:?} is unlabelled, but a {f} to a {t} is {} relations",
                    relations.len()
                );
                unlabelled.insert((diagram.to_string(), relation.clone()));
            }
        }
    }
    for kind in [
        "actor",
        "usecase",
        "screen",
        "api",
        "entity",
        "extsystem",
        "event",
        "timer",
        "condition",
        "goal",
        "requirement",
        "buc",
        "state",
        "statemodel",
        "value",
        "step",
        "system",
    ] {
        assert!(kinds.contains(kind), "no sample draws a {kind}");
    }
    for (diagram, relation) in [
        ("rdra", "performs"),
        ("requirement", "achieved_by"),
        ("usecase", "displays"),
        ("robustness", "uses"),
        ("event", "raises"),
        ("flow", "covers"),
        ("information", "statemodel"),
        ("condition", "applies_to"),
    ] {
        assert!(
            unlabelled.contains(&(diagram.to_string(), relation.to_string())),
            "no unlabelled {relation} in any {diagram} diagram"
        );
    }
}

#[test]
fn an_element_has_one_id_in_every_scope() {
    let dir = scratch("diagram_contract_scopes");
    let root = "samples/ec-site";
    let whole = parse_flowchart("rdra", &diagram(root, "rdra", None, &dir.join("w"))).unwrap();
    for buc in buc_ids(root) {
        let scoped =
            parse_flowchart("rdra", &diagram(root, "rdra", Some(&buc), &dir.join("b"))).unwrap();
        for id in scoped.nodes.keys() {
            assert!(whole.nodes.contains_key(id), "{buc}: {id} not in whole");
        }
    }
}

#[test]
fn every_state_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_state");
    let mut drawn = 0;
    for root in lifecycle_roots() {
        let scopes: Vec<Option<String>> = std::iter::once(None)
            .chain(buc_ids(&root).into_iter().map(Some))
            .collect();
        for buc in scopes {
            let text = diagram(&root, "state", buc.as_deref(), &dir.join("out"));
            let chart = parse_state(&text)
                .and_then(|chart| check_state(&chart).map(|()| chart))
                .unwrap_or_else(|error| panic!("{root} {buc:?}: {error}\n{text}"));
            drawn += usize::from(!chart.models.is_empty());
        }
    }
    assert!(drawn > 10, "only {drawn} state diagrams draw a state model");
}

#[test]
fn the_unscoped_state_diagram_is_an_index_and_each_entity_draws_its_models() {
    let dir = scratch("diagram_contract_state_entity");
    let mut full = 0;
    for root in lifecycle_roots() {
        let text = diagram(&root, "state", None, &dir.join("index"));
        let index = parse_state(&text)
            .and_then(|chart| check_state(&chart).map(|()| chart))
            .unwrap_or_else(|error| panic!("{root}: {error}\n{text}"));
        assert_eq!(index.index_notes.len(), index.models.len(), "{root}");
        for entity in indexed_entities(&index) {
            let text = diagram_with(&root, "state", &["--entity", &entity], &dir.join("full"));
            let chart = parse_state(&text)
                .and_then(|chart| check_state(&chart).map(|()| chart))
                .unwrap_or_else(|error| panic!("{root} {entity}: {error}\n{text}"));
            assert!(chart.index_notes.is_empty(), "{root} {entity}");
            assert!(chart
                .models
                .iter()
                .all(|(_, l)| l.starts_with(&format!("{entity}."))));
            for (model, label) in &index.index_notes {
                let Some((_, at)) = chart.models.iter().find(|(id, _)| id == model) else {
                    continue;
                };
                let moves = chart.transitions.iter().filter(|t| &t.0 == model).count();
                let drawn = chart.states.values().filter(|m| *m == model).count();
                let (n, rest) = label.split_once(" 状態 / ").unwrap();
                let m = rest.split_once(" 遷移").unwrap().0;
                assert_eq!(m.parse::<usize>().unwrap(), moves, "{root} {at}");
                assert!(n.parse::<usize>().unwrap() >= drawn, "{root} {at}");
            }
            full += 1;
        }
    }
    assert!(full > 5, "only {full} entity diagrams");
}

/// An Enum column a `sets` writes is a state axis of `check` even without `transitions`: the
/// state diagram names it on `%% set-only:`, not on `omitted:` (#61).
#[test]
fn a_state_axis_set_by_sets_is_not_omitted() {
    let dir = scratch("diagram_contract_state_set_only");
    let mut set_only = 0;
    for root in lifecycle_roots() {
        let text = diagram(&root, "state", None, &dir.join("index"));
        let (omitted, set) = (
            header_columns(&text, "omitted"),
            header_columns(&text, "set-only"),
        );
        assert!(omitted.is_disjoint(&set), "{root}\n{text}");
        set_only += set.len();
        if root == "samples/patent-ops" {
            for column in ["OfficeAction.kind", "Patent.last_payment"] {
                assert!(set.contains(column), "{root}: {column}\n{text}");
            }
        }
    }
    assert!(set_only >= 2, "only {set_only} set-only columns");
}

#[test]
fn the_state_contract_check_rejects_a_broken_diagram() {
    let good = "%% rdra-ish diagram=state scope=entity:Doc\n%% kinds: statemodel state usecase\n%% legend: stateunreachable = a, statedeadlock = b, stateend = c\nstateDiagram-v2\n  state \"Doc.status\" as statemodel__Doc_status {\n    [*] --> state__Doc_draft\n    state \"draft\" as state__Doc_draft\n    state \"done\" as state__Doc_done\n    %% transition usecase__Publish\n    state__Doc_draft --> state__Doc_done : Publish\n    state__Doc_done --> [*]\n  }\n  classDef stateend fill:#dfd\n  class state__Doc_done stateend\n";
    assert!(check_state(&parse_state(good).unwrap()).is_ok());
    for (broken, from, to) in [
        ("missing end", "    state__Doc_done --> [*]\n", ""),
        (
            "end with a way out",
            "    state__Doc_done --> [*]\n",
            "    state__Doc_done --> [*]\n    state__Doc_draft --> [*]\n",
        ),
        (
            "transition is not an id",
            "%% transition usecase__Publish",
            "%% transition Publish",
        ),
        (
            "transition without its id",
            "    %% transition usecase__Publish\n",
            "",
        ),
        ("emoji-era id", "as state__Doc_done", "as Doc_done"),
        (
            "no legend",
            "%% legend: stateunreachable = a, statedeadlock = b, stateend = c\n",
            "",
        ),
        ("no classDef", "  classDef stateend fill:#dfd\n", ""),
        (
            "end without its class",
            "  class state__Doc_done stateend\n",
            "",
        ),
        (
            "end class on a state with a way out",
            "  class state__Doc_done stateend\n",
            "  class state__Doc_done stateend\n  class state__Doc_draft stateend\n",
        ),
        (
            "deadlock on a state with no way out",
            "  class state__Doc_done stateend\n",
            "  class state__Doc_done statedeadlock\n",
        ),
        (
            "unknown class",
            "  class state__Doc_done stateend\n",
            "  class state__Doc_done stateend\n  class state__Doc_draft stateweird\n",
        ),
        (
            "header kinds",
            "statemodel state usecase",
            "statemodel state",
        ),
    ] {
        let text = good.replace(from, to);
        assert_ne!(text, good, "{broken}");
        let result = parse_state(&text).and_then(|chart| check_state(&chart));
        assert!(result.is_err(), "{broken} passed:\n{text}");
    }
}

#[test]
fn a_state_and_its_use_cases_have_the_ids_the_rdra_diagram_gives_them() {
    let dir = scratch("diagram_contract_state_ids");
    for root in lifecycle_roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let index = parse_state(&diagram(&root, "state", None, &dir.join("i"))).unwrap();
        let ids: Vec<String> = indexed_entities(&index)
            .iter()
            .flat_map(|entity| {
                let text = diagram_with(&root, "state", &["--entity", entity], &dir.join("s"));
                let state = parse_state(&text).unwrap();
                let usecases: Vec<String> = state.transitions.iter().map(|t| t.3.clone()).collect();
                state.states.into_keys().chain(usecases).collect::<Vec<_>>()
            })
            .collect();
        for id in &ids {
            assert!(rdra.nodes.contains_key(id), "{root}: {id} not in rdra");
        }
    }
}

#[test]
fn every_event_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_event");
    let mut drawn = 0;
    for root in roots() {
        let scopes: Vec<Option<String>> = std::iter::once(None)
            .chain(buc_ids(&root).into_iter().map(Some))
            .collect();
        for buc in scopes {
            let text = diagram(&root, "event", buc.as_deref(), &dir.join("out"));
            let chart = parse_flowchart("event", &text)
                .and_then(|chart| check_flowchart(&chart).map(|()| chart))
                .unwrap_or_else(|error| panic!("{root} {buc:?}: {error}\n{text}"));
            for (from, label, to) in &chart.edges {
                assert!(
                    label == "raises" || label == "triggers",
                    "{root} {buc:?}: {from} -{label}-> {to}"
                );
            }
            assert!(
                !chart.nodes.values().any(|kind| kind == "state"),
                "{root} {buc:?}: a state node\n{text}"
            );
            drawn += usize::from(!chart.edges.is_empty());
        }
    }
    assert!(drawn > 10, "only {drawn} event diagrams draw an edge");
}

#[test]
fn an_event_diagram_node_has_the_id_the_rdra_diagram_gives_it() {
    let dir = scratch("diagram_contract_event_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let event =
            parse_flowchart("event", &diagram(&root, "event", None, &dir.join("e"))).unwrap();
        for (id, kind) in &event.nodes {
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge in &event.edges {
            assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
        }
    }
}

#[test]
fn every_usecase_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_usecase");
    let mut derived = 0;
    for root in roots() {
        let mut scopes = scopes(&root);
        scopes.extend(
            ids(&root, "usecase")
                .into_iter()
                .take(3)
                .map(|usecase| vec!["--usecase".to_string(), usecase]),
        );
        for scope in scopes {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "usecase", &args, &dir.join("out"));
            let chart = checked("usecase", &text, &format!("{root} {scope:?}"));
            for (from, label, to) in &chart.edges {
                let usecase_end = from.starts_with("usecase__") || to.starts_with("usecase__");
                let screen_shows = label == "shows" && from.starts_with("screen__");
                let event_link = from.starts_with("event__") || to.starts_with("event__");
                assert!(
                    usecase_end || screen_shows || event_link,
                    "{root} {scope:?}: {from} -{label}-> {to}"
                );
            }
            for kind in ["api", "dto", "permission", "medium", "field", "buc"] {
                assert!(
                    !chart.nodes.values().any(|k| k == kind),
                    "{root} {scope:?}: a {kind} node\n{text}"
                );
            }
            derived += chart.derived.len();
        }
    }
    assert!(derived > 20, "only {derived} derived edges");
}
