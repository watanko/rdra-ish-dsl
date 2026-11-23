use super::*;

#[test]
fn an_information_diagram_node_has_the_id_the_rdra_and_state_diagrams_give_it() {
    let dir = scratch("diagram_contract_information_ids");
    for root in lifecycle_roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let state = parse_state(&diagram(&root, "state", None, &dir.join("s"))).unwrap();
        let info = parse_flowchart(
            "information",
            &diagram(&root, "information", None, &dir.join("i")),
        )
        .unwrap();
        let state_models: BTreeSet<&String> = state.models.iter().map(|(id, _)| id).collect();
        let info_models: BTreeSet<&String> = info
            .nodes
            .iter()
            .filter(|(_, kind)| *kind == "statemodel")
            .map(|(id, _)| id)
            .collect();
        assert_eq!(info_models, state_models, "{root}");
        for (id, kind) in info.nodes.iter().filter(|(_, kind)| *kind != "statemodel") {
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge in info
            .edges
            .iter()
            .filter(|(_, _, to)| to.starts_with("entity__"))
        {
            assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
        }
    }
}

#[test]
fn every_condition_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_condition");
    let (mut derived, mut values) = (0, 0);
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "condition", &args, &dir.join("out"));
            let chart = checked("condition", &text, &format!("{root} {scope:?}"));
            for (from, label, to) in &chart.edges {
                let fits = match label.as_str() {
                    "combines" => to.starts_with("variation__") || to.starts_with("statemodel__"),
                    "applies_to" => ["usecase__", "api__", "system__"]
                        .iter()
                        .any(|kind| to.starts_with(kind)),
                    "when" => to.starts_with("usecase__"),
                    _ => false,
                };
                assert!(
                    fits && from.starts_with("condition__"),
                    "{root} {scope:?}: {from} -{label}-> {to}"
                );
            }
            for (id, kind) in &chart.nodes {
                let variation = id.rsplit_once("__").map(|(variation, _)| variation);
                assert!(
                    kind != "value" || variation.is_some_and(|v| chart.nodes.contains_key(v)),
                    "{root} {scope:?}: value {id} outside its variation"
                );
            }
            derived += chart.derived.len();
            values += chart.nodes.values().filter(|kind| *kind == "value").count();
        }
    }
    assert!(derived >= 8, "only {derived} combines read from holds");
    assert!(values > 40, "only {values} values drawn");
}

/// A `.when(Cond)` on a `transitions` arm, a `chooses` value, or a `sets` value is drawn as a `when` edge from
/// the condition to the use case in each diagram that draws `applies_to`, labelled because a
/// condition to a use case is two relations there, and derived from the arms and values it
/// guards: the same edges in every kind.
#[test]
fn a_when_guard_is_a_labelled_derived_edge_wherever_applies_to_is_drawn() {
    let dir = scratch("diagram_contract_when");
    let mut guards = 0;
    for root in roots() {
        let mut drawn: Vec<BTreeSet<(String, String, String)>> = Vec::new();
        for kind in ["rdra", "usecase", "condition"] {
            assert_eq!(
                rdra_ish_emit::implied_relation(diagram_kind(kind), "condition", "usecase"),
                None,
                "{kind}: a condition to a use case is `applies_to` or `when`"
            );
            let text = diagram(&root, kind, None, &dir.join(kind));
            let chart = checked(kind, &text, &format!("{root} {kind}"));
            let when: BTreeSet<_> = chart
                .edges
                .iter()
                .filter(|(_, label, _)| label == "when")
                .cloned()
                .collect();
            for edge @ (from, _, to) in &when {
                assert!(
                    from.starts_with("condition__") && to.starts_with("usecase__"),
                    "{root} {kind}: {edge:?}"
                );
                assert!(chart.derived.contains(edge), "{root} {kind}: {edge:?}");
                assert_eq!(chart.arrows[edge], "-.->", "{root} {kind}: {edge:?}");
                let vias = derived_vias(&text, from, to);
                assert!(
                    !vias.is_empty()
                        && vias.iter().all(|via| {
                            let via = via.trim();
                            via.starts_with("transitions ")
                                || via.starts_with("chooses ")
                                || via.starts_with("sets ")
                        }),
                    "{root} {kind}: {edge:?} via {vias:?}"
                );
            }
            drawn.push(when);
        }
        assert!(
            drawn.windows(2).all(|pair| pair[0] == pair[1]),
            "{root}: rdra, usecase, and condition draw different `when` edges: {drawn:?}"
        );
        guards += drawn[0].len();
    }
    assert!(guards >= 2, "only {guards} `when` edges drawn");
}

#[test]
fn the_contract_check_rejects_a_misplaced_value() {
    let good = "%% rdra-ish diagram=condition scope=whole\n%% kinds: variation value\nflowchart LR\n  subgraph variation__V[\"V\"]\n    variation__V__a[\"A\"]:::value\n  end\n  classDef variation stroke:#333;\n  classDef value stroke:#333;\n";
    assert!(check_flowchart(&parse_flowchart("condition", good).unwrap()).is_ok());
    for (broken, from, to) in [
        ("no variation", "variation__V__a[", "variation__a["),
        ("another kind", "variation__V__a[", "condition__V__a["),
        ("untagged value", "\"]:::value", "\"]:::variation"),
    ] {
        let text = good.replace(from, to);
        assert_ne!(text, good, "{broken}");
        let result = parse_flowchart("condition", &text).and_then(|chart| check_flowchart(&chart));
        assert!(result.is_err(), "{broken} passed:\n{text}");
    }
}

#[test]
fn a_condition_diagram_node_has_the_id_the_rdra_and_state_diagrams_give_it() {
    let dir = scratch("diagram_contract_condition_ids");
    for root in lifecycle_roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let state = parse_state(&diagram(&root, "state", None, &dir.join("s"))).unwrap();
        let condition = parse_flowchart(
            "condition",
            &diagram(&root, "condition", None, &dir.join("c")),
        )
        .unwrap();
        for (id, kind) in &condition.nodes {
            match kind.as_str() {
                "statemodel" => assert!(
                    state.models.iter().any(|(model, _)| model == id),
                    "{root}: {id} not in state"
                ),
                "value" => {}
                _ => assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}"),
            }
        }
        for edge in condition
            .edges
            .iter()
            .filter(|(_, _, to)| !to.starts_with("statemodel__"))
        {
            assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
            assert_eq!(
                condition.derived.contains(edge),
                rdra.derived.contains(edge),
                "{root}: {edge:?} derived in one diagram only"
            );
        }
    }
}

#[test]
fn every_flow_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_flow");
    let (mut drawn, mut orders) = (0, BTreeSet::new());
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "flow", &args, &dir.join("out"));
            let chart = checked("flow", &text, &format!("{root} {scope:?}"));
            check_flow(&chart).unwrap_or_else(|error| panic!("{root} {scope:?}: {error}\n{text}"));
            drawn += usize::from(chart.nodes.values().any(|kind| kind == "flow"));
            orders.extend(chart.edges.iter().map(|(_, label, to)| {
                let kind = to.split_once("__").map_or("", |(kind, _)| kind);
                format!("{label} {kind}")
            }));
        }
    }
    assert!(drawn >= 5, "only {drawn} flow diagrams draw a flow");
    for edge in [
        "precedes step",
        "branches step",
        "excepts step",
        "repeats step",
        "covers usecase",
        "covers event",
        "covers timer",
    ] {
        assert!(orders.contains(edge), "no `{edge}` edge in any sample");
    }
}

#[test]
fn a_flow_diagram_with_lanes_keeps_the_contract() {
    let dir = scratch("diagram_contract_flow_lanes");
    let model = dir.join("lanes.rdra");
    std::fs::write(&model, LANES).unwrap();
    let root = model.to_str().unwrap();
    let text = diagram(root, "flow", None, &dir.join("out"));
    let chart = checked("flow", &text, "lanes");
    check_flow(&chart).unwrap_or_else(|error| panic!("{error}\n{text}"));
    let lanes: BTreeSet<&str> = chart
        .nodes
        .iter()
        .filter(|(_, kind)| *kind == "lane")
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        lanes,
        BTreeSet::from(["lane__ReturnFlow__Clerk", "lane__ShipFlow__Clerk"]),
        "{text}"
    );
    assert!(
        text.contains("    subgraph lane__ShipFlow__Clerk[\"Clerk\"]\n      %% lane lane__ShipFlow__Clerk: actor__Clerk\n"),
        "{text}"
    );
    let rdra = parse_flowchart("rdra", &diagram(root, "rdra", None, &dir.join("r"))).unwrap();
    for edge in &chart.edges {
        assert!(rdra.edges.contains(edge), "{edge:?} not in rdra");
    }
    for broken in ["lane__NoFlow__Clerk", "lane__ShipFlow"] {
        let text = text.replace("lane__ShipFlow__Clerk[", &format!("{broken}["));
        let result = parse_flowchart("flow", &text).and_then(|chart| check_flow(&chart));
        assert!(result.is_err(), "{broken} passed:\n{text}");
    }
}

#[test]
fn a_flow_diagram_draws_what_the_rdra_diagram_declares() {
    let dir = scratch("diagram_contract_flow_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let flow = parse_flowchart("flow", &diagram(&root, "flow", None, &dir.join("f"))).unwrap();
        for (id, kind) in flow.nodes.iter().filter(|(_, kind)| *kind != "lane") {
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge in &flow.edges {
            assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
        }
        assert!(flow.derived.is_empty(), "{root}: a derived edge");
    }
}

#[test]
fn every_context_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_context");
    let mut edges = 0;
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "context", &args, &dir.join("out"));
            let chart = checked("context", &text, &format!("{root} {scope:?}"));
            for kind in chart.nodes.values() {
                assert!(
                    ["actor", "extsystem", "system"].contains(&kind.as_str()),
                    "{root} {scope:?}: a {kind} node\n{text}"
                );
            }
            assert!(
                !text.contains("subgraph"),
                "{root} {scope:?}: a subgraph\n{text}"
            );
            assert!(
                !text.contains("%% layers:"),
                "{root} {scope:?}: a layers line\n{text}"
            );
            let mut pairs = BTreeSet::new();
            for edge @ (from, label, to) in &chart.edges {
                // One edge per outside node, system, and direction, labelled with what it
                // was derived through: an actor's BUCs (a `performs`), an external
                // system's events.
                let via_kinds: &[&str] = if from.starts_with("actor__") {
                    &["buc__", "usecase__", "step__"]
                } else if from.starts_with("extsystem__") || to.starts_with("extsystem__") {
                    &["event__"]
                } else {
                    &[]
                };
                let system_end = from.starts_with("system__") || to.starts_with("system__");
                let vias = derived_vias(&text, from, to);
                assert!(
                    !via_kinds.is_empty()
                        && system_end
                        && !label.is_empty()
                        && !chart.unlabelled.contains(edge)
                        && chart.derived.contains(edge)
                        && !vias.is_empty()
                        && vias
                            .iter()
                            .all(|via| via_kinds.iter().any(|kind| via.starts_with(kind)))
                        && pairs.insert((from.clone(), to.clone())),
                    "{root} {scope:?}: {from} -{label}-> {to}\n{text}"
                );
            }
            edges += chart.edges.len();
        }
    }
    assert!(edges > 50, "only {edges} context edges");
}

#[test]
fn a_context_diagram_node_has_the_id_the_rdra_diagram_gives_it() {
    let dir = scratch("diagram_contract_context_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let text = diagram(&root, "context", None, &dir.join("c"));
        let context = parse_flowchart("context", &text).unwrap();
        for (id, kind) in &context.nodes {
            if id == "system__Model" {
                assert!(
                    text.contains("  %% synthesized system__Model: "),
                    "{root}: system__Model without its reason"
                );
                continue;
            }
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for line in text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("%% derived "))
        {
            let (_, vias) = line.split_once(" via ").unwrap();
            for via in vias.split(',') {
                assert!(
                    rdra.nodes.contains_key(via),
                    "{root}: via {via} not in rdra"
                );
            }
        }
    }
}

#[test]
fn every_requirement_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_requirement");
    let mut labels = BTreeSet::new();
    let mut apart_seen = BTreeSet::new();
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "requirement", &args, &dir.join("out"));
            let chart = checked("requirement", &text, &format!("{root} {scope:?}"));
            // A value tree: goals, requirements, and BUCs, and no layer subgraph.
            for kind in chart.nodes.values() {
                assert!(
                    ["goal", "requirement", "buc"].contains(&kind.as_str()),
                    "{root} {scope:?}: a {kind} node\n{text}"
                );
            }
            assert!(
                !text.contains("subgraph layer_") && !text.contains("%% layers:"),
                "{root} {scope:?}: a layer\n{text}"
            );
            for (from, label, to) in &chart.edges {
                let source = from.split_once("__").map_or("", |(kind, _)| kind);
                let fits = match label.as_str() {
                    "motivates" => {
                        source == "requirement"
                            && to.starts_with("buc__")
                            && !chart
                                .derived
                                .contains(&(from.clone(), label.clone(), to.clone()))
                    }
                    // `achieves(Requirement, Goal)` read from the goal: always derived, and
                    // `via` names the requirement the edge ends at.
                    "achieved_by" => {
                        source == "goal"
                            && to.starts_with("requirement__")
                            && derived_vias(&text, from, to) == [to.clone()]
                    }
                    _ => false,
                };
                assert!(fits, "{root} {scope:?}: {from} -{label}-> {to}\n{text}");
                labels.insert(label.clone());
            }
            // A requirement apart has only dashed edges, and any other only solid ones.
            let apart = apart_requirements(&text);
            for (edge @ (from, ..), arrow) in &chart.arrows {
                if from.starts_with("goal__") {
                    continue;
                }
                assert_eq!(
                    arrow == "-.->",
                    apart.contains(from),
                    "{root} {scope:?}: {edge:?}\n{text}"
                );
            }
            for id in &apart {
                assert!(
                    chart
                        .nodes
                        .get(id)
                        .is_some_and(|kind| kind == "requirement"),
                    "{root} {scope:?}: {id} apart is not a requirement"
                );
            }
            apart_seen.extend(apart);
        }
    }
    assert_eq!(
        labels,
        BTreeSet::from(["achieved_by", "motivates"].map(String::from))
    );
    assert!(
        apart_seen.contains("requirement__ReqNoOnlineConsultation"),
        "no sample draws a requirement apart: {apart_seen:?}"
    );
}

#[test]
fn a_requirement_diagram_draws_what_the_rdra_diagram_declares() {
    let dir = scratch("diagram_contract_requirement_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let requirement = parse_flowchart(
            "requirement",
            &diagram(&root, "requirement", None, &dir.join("q")),
        )
        .unwrap();
        for (id, kind) in &requirement.nodes {
            // The out-of-scope subgraph is no element; its members are.
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge @ (from, label, to) in &requirement.edges {
            // A derived `achieved_by` is the declared `achieves`, read from the goal.
            let declared = if requirement.derived.contains(edge) {
                assert_eq!(label, "achieved_by", "{root}: {edge:?}");
                (to.clone(), "achieves".to_string(), from.clone())
            } else {
                edge.clone()
            };
            assert!(
                rdra.edges.contains(&declared),
                "{root}: {edge:?} not in rdra"
            );
        }
    }
}

#[test]
fn every_screen_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_screen");
    let (mut solid, mut dashed) = (0, 0);
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "screen", &args, &dir.join("out"));
            let chart = checked("screen", &text, &format!("{root} {scope:?}"));
            assert!(
                chart.nodes.values().all(|kind| kind == "screen"),
                "{root} {scope:?}\n{text}"
            );
            for edge in &chart.edges {
                // A dashed edge is the business order, derived; a solid one is a navigates.
                let derived = chart.derived.contains(edge);
                if arrow_of(&chart, edge) == "-.->" {
                    assert!(
                        derived && edge.1 == "業務上の順番",
                        "{root} {scope:?}: {edge:?}"
                    );
                    dashed += 1;
                } else {
                    assert!(!derived, "{root} {scope:?}: {edge:?}");
                    solid += 1;
                }
            }
        }
    }
    assert!(
        solid > 5 && dashed > 5,
        "{solid} solid, {dashed} dashed edges"
    );
}

#[test]
fn a_screen_diagram_draws_the_screens_and_navigates_of_the_rdra_diagram() {
    let dir = scratch("diagram_contract_screen_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let screen =
            parse_flowchart("screen", &diagram(&root, "screen", None, &dir.join("s"))).unwrap();
        let rdra_screens: BTreeSet<&String> = rdra
            .nodes
            .iter()
            .filter(|(_, kind)| *kind == "screen")
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            screen.nodes.keys().collect::<BTreeSet<_>>(),
            rdra_screens,
            "{root}"
        );
        for edge @ (from, _, to) in &screen.edges {
            if !screen.derived.contains(edge) {
                let navigates = (from.clone(), "navigates".to_string(), to.clone());
                assert!(
                    rdra.edges.contains(&navigates),
                    "{root}: {edge:?} not in rdra"
                );
            }
        }
    }
}

#[test]
fn lint_reports_each_screen_the_screen_diagram_leaves_unconnected() {
    let dir = scratch("diagram_contract_screen_lint");
    let mut reported = 0;
    for root in roots() {
        let chart =
            parse_flowchart("screen", &diagram(&root, "screen", None, &dir.join("s"))).unwrap();
        let unconnected: BTreeSet<String> = chart
            .nodes
            .keys()
            .filter(|id| {
                !chart
                    .edges
                    .iter()
                    .any(|(from, _, to)| from == *id || to == *id)
            })
            .map(|id| id.trim_start_matches("screen__").to_string())
            .collect();
        let linted = lint_subjects(&root, "screen-no-transition");
        assert_eq!(linted, unconnected, "{root}");
        reported += linted.len();
    }
    assert!(reported > 0, "no sample has an unconnected screen");
}

#[test]
fn clinic_ops_screen_transitions_cover_check_in_encounter_and_payment() {
    let dir = scratch("diagram_contract_screen_clinic_ops");
    let text = diagram("samples/clinic-ops", "screen", None, &dir.join("s"));
    let chart = parse_flowchart("screen", &text).unwrap();
    for (from, label, to) in [
        ("CheckInScreen", "受付（初診・再診）", "EligibilityScreen"),
        (
            "EncounterWorkspaceScreen",
            "診察・診断の記録",
            "DiagnosisScreen",
        ),
        ("BillCalculationScreen", "会計の呼出", "PaymentWindowScreen"),
    ] {
        let edge = (
            format!("screen__{from}"),
            label.to_string(),
            format!("screen__{to}"),
        );
        assert!(chart.edges.contains(&edge), "{edge:?}\n{text}");
        assert_eq!(arrow_of(&chart, &edge), "-->", "{edge:?}");
    }
    assert!(!chart.derived.is_empty(), "{text}");
}
