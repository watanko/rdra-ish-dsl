use super::*;

#[test]
fn a_usecase_diagram_draws_what_the_rdra_diagram_declares() {
    let dir = scratch("diagram_contract_usecase_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let usecase =
            parse_flowchart("usecase", &diagram(&root, "usecase", None, &dir.join("u"))).unwrap();
        for (id, kind) in &usecase.nodes {
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge in &usecase.edges {
            if !usecase.derived.contains(edge) {
                assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
            }
        }
    }
}

#[test]
fn every_robustness_diagram_of_every_sample_keeps_the_contract_and_the_grammar() {
    let dir = scratch("diagram_contract_robustness");
    let (mut edges, mut timers) = (0, 0);
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
            let text = diagram_with(&root, "robustness", &args, &dir.join("out"));
            let chart = checked("robustness", &text, &format!("{root} {scope:?}"));
            let kind = |id: &String| chart.nodes[id].as_str();
            let control = |id: &String| matches!(kind(id), "api" | "usecase");
            let boundary = |id: &String| matches!(kind(id), "screen" | "extsystem" | "timer");
            for (from, label, to) in &chart.edges {
                let ok = match kind(from) {
                    "actor" => kind(to) == "screen",
                    "screen" => control(to),
                    "api" | "usecase" => {
                        matches!(kind(to), "entity") || control(to) || boundary(to)
                    }
                    "extsystem" | "timer" => control(to),
                    _ => false,
                };
                assert!(
                    ok && kind(from) != "entity",
                    "{root} {scope:?}: {from} -{label}-> {to}"
                );
                if kind(to) == "entity" {
                    assert!(control(from), "{root} {scope:?}: {from} -> {to}");
                }
                // A timer starts the use case, the one relation from a timer (#61, #43).
                if kind(from) == "timer" {
                    let edge = (from.clone(), label.clone(), to.clone());
                    assert!(
                        label == "triggers" && chart.unlabelled.contains(&edge),
                        "{root} {scope:?}: {from} -{label}-> {to}"
                    );
                    timers += 1;
                }
            }
            for kind in [
                "dto",
                "permission",
                "medium",
                "field",
                "buc",
                "event",
                "state",
            ] {
                assert!(
                    !chart.nodes.values().any(|k| k == kind),
                    "{root} {scope:?}: a {kind} node\n{text}"
                );
            }
            edges += chart.edges.len();
        }
    }
    assert!(edges > 100, "only {edges} edges");
    assert!(timers > 5, "only {timers} timer edges");
}

#[test]
fn every_sequence_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_sequence");
    let mut drawn = 0;
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        for usecase in ids(&root, "usecase").into_iter().take(4) {
            let text = diagram_with(
                &root,
                "sequence",
                &["--usecase", &usecase],
                &dir.join("out"),
            );
            let participants = parse_sequence(&text, &format!("usecase:{usecase}"))
                .unwrap_or_else(|why| panic!("{root} {usecase}: {why}\n{text}"));
            for id in &participants {
                assert!(
                    rdra.nodes.contains_key(id),
                    "{root} {usecase}: {id} not in rdra"
                );
            }
            drawn += participants.len();
        }
    }
    assert!(drawn > 20, "only {drawn} participants");
}

#[test]
fn a_sequence_diagram_needs_exactly_one_use_case() {
    let root = &roots()[0];
    let usecases = ids(root, "usecase");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(repo())
            .args(["diagram", root, "--kind", "sequence"])
            .args(args)
            .arg("--out")
            .arg(scratch("diagram_contract_sequence_args").join("out"))
            .output()
            .unwrap()
    };
    let none = run(&[]);
    assert!(!none.status.success());
    assert!(String::from_utf8_lossy(&none.stderr).contains("exactly one --usecase"));
    let two = run(&["--usecase", &usecases[0], "--usecase", &usecases[1]]);
    assert!(!two.status.success());
}

/// `sequence --buc` plays the BUC's use cases one after another, each under a numbered note,
/// in one diagram that keeps the sequence grammar and draws only nodes the rdra diagram has.
#[test]
fn a_sequence_diagram_of_a_buc_plays_its_use_cases_in_one_diagram() {
    let dir = scratch("diagram_contract_sequence_buc");
    let mut played = 0;
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        for buc in buc_ids(&root).into_iter().take(2) {
            let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
                .current_dir(repo())
                .args(["diagram", &root, "--kind", "sequence", "--buc", &buc])
                .output()
                .unwrap();
            // A BUC without use cases has no sequence to draw.
            if !output.status.success() {
                continue;
            }
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let participants = parse_sequence(&text, &format!("buc:{buc}"))
                .unwrap_or_else(|why| panic!("{root} {buc}: {why}\n{text}"));
            for id in &participants {
                assert!(
                    rdra.nodes.contains_key(id),
                    "{root} {buc}: {id} not in rdra"
                );
            }
            let notes: Vec<&str> = text
                .lines()
                .filter_map(|l| l.trim().strip_prefix("Note over "))
                .filter_map(|l| l.split_once(": ").map(|(_, said)| said))
                .filter(|said| {
                    said.split_once(". ")
                        .is_some_and(|(n, _)| n.parse::<u32>().is_ok())
                })
                .collect();
            for (i, note) in notes.iter().enumerate() {
                assert!(
                    note.starts_with(&format!("{}. ", i + 1)),
                    "{root} {buc}: {notes:?}"
                );
            }
            played += notes.len();
        }
    }
    assert!(played > 10, "only {played} use cases played");
}

#[test]
fn the_contract_check_rejects_a_misplaced_derived_line() {
    let good = "%% rdra-ish diagram=usecase scope=whole\n%% kinds: actor usecase\nflowchart LR\n  actor__A([\"A\"]):::actor\n  usecase__U(\"U\"):::usecase\n  actor__A -.-> usecase__U\n  %% derived actor__A -> usecase__U via buc__B\n  classDef actor stroke:#333;\n  classDef usecase stroke:#333;\n";
    let chart = parse_flowchart("usecase", good).unwrap();
    assert_eq!(chart.derived.len(), 1);
    let broken = good.replace(
        "%% derived actor__A -> usecase__U",
        "%% derived usecase__U -> actor__A",
    );
    assert!(parse_flowchart("usecase", &broken).is_err(), "{broken}");
}

#[test]
fn every_business_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_business");
    let mut drawn = 0;
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "business", &args, &dir.join("out"));
            let chart = checked("business", &text, &format!("{root} {scope:?}"));
            // A business flow: businesses holding BUCs, and no actor, external system, or
            // layer.
            for kind in chart.nodes.values() {
                assert!(
                    ["business", "buc"].contains(&kind.as_str()),
                    "{root} {scope:?}: a {kind} node\n{text}"
                );
            }
            assert!(
                !text.contains("subgraph layer_") && !text.contains("%% layers:"),
                "{root} {scope:?}: a layer\n{text}"
            );
            let mut handoffs = BTreeSet::new();
            for edge @ (from, label, to) in &chart.edges {
                if label == "belongs" {
                    // A BUC of several businesses sits in one and belongs to the others.
                    assert!(
                        from.starts_with("buc__")
                            && to.starts_with("business__")
                            && !chart.derived.contains(edge),
                        "{root} {scope:?}: {from} -{label}-> {to}\n{text}"
                    );
                    continue;
                }
                // A hand-off: BUC to another BUC, named by the event label, derived through
                // the event ids.
                let vias = derived_vias(&text, from, to);
                assert!(
                    from.starts_with("buc__")
                        && to.starts_with("buc__")
                        && from != to
                        && chart.derived.contains(edge)
                        && !vias.is_empty()
                        && vias.iter().all(|via| via.starts_with("event__"))
                        && handoffs.insert((from.clone(), label.clone(), to.clone())),
                    "{root} {scope:?}: {from} -{label}-> {to}\n{text}"
                );
            }
            drawn += handoffs.len();
        }
    }
    assert!(drawn > 10, "only {drawn} hand-offs drawn");
}

#[test]
fn a_business_diagram_draws_what_the_rdra_diagram_declares() {
    let dir = scratch("diagram_contract_business_ids");
    for root in roots() {
        let rdra = parse_flowchart("rdra", &diagram(&root, "rdra", None, &dir.join("r"))).unwrap();
        let text = diagram(&root, "business", None, &dir.join("b"));
        let business = parse_flowchart("business", &text).unwrap();
        for (id, kind) in &business.nodes {
            assert_eq!(rdra.nodes.get(id), Some(kind), "{root}: {id}");
        }
        for edge @ (from, _, to) in &business.edges {
            if business.derived.contains(edge) {
                // A hand-off: a use case of `from` raises each event and it triggers a use
                // case of `to` or `to` itself, which the rdra diagram draws.
                for event in derived_vias(&text, from, to) {
                    let raised = rdra
                        .edges
                        .iter()
                        .any(|(_, label, target)| label == "raises" && *target == event);
                    let triggers = rdra
                        .edges
                        .iter()
                        .any(|(source, label, _)| label == "triggers" && *source == event);
                    assert!(
                        raised && triggers,
                        "{root}: {event} of {edge:?} is not raised and triggered in rdra"
                    );
                }
            } else {
                assert!(rdra.edges.contains(edge), "{root}: {edge:?} not in rdra");
            }
        }
    }
}

#[test]
fn clinic_ops_business_flow_has_its_eight_hand_offs() {
    let dir = scratch("diagram_contract_business_clinic_ops");
    let text = diagram("samples/clinic-ops", "business", None, &dir.join("b"));
    let chart = parse_flowchart("business", &text).unwrap();
    let handoffs = chart
        .edges
        .iter()
        .filter(|(_, label, _)| label != "belongs")
        .count();
    assert_eq!(handoffs, 8, "{text}");
    assert!(
        chart.edges.contains(&(
            "buc__BucVisitCheckIn".to_string(),
            "受付完了".to_string(),
            "buc__BucClinicalEncounter".to_string()
        )),
        "{text}"
    );
}

#[test]
fn an_element_subgraph_is_a_node_of_its_kind() {
    let good = "%% rdra-ish diagram=business scope=whole\n%% kinds: business buc\nflowchart LR\n  subgraph business__B[\"B\"]\n    buc__U[\"U\"]:::buc\n  end\n  classDef business stroke:#333;\n  classDef buc stroke:#333;\n";
    let chart = parse_flowchart("business", good).unwrap();
    assert!(check_flowchart(&chart).is_ok());
    assert_eq!(
        chart.nodes.get("business__B").map(String::as_str),
        Some("business")
    );
    for (broken, from, to) in [
        ("no kind", "subgraph business__B[", "subgraph B["),
        (
            "unterminated",
            "business__B[\"B\"]\n",
            "business__B[\"B\"\n",
        ),
    ] {
        let text = good.replace(from, to);
        assert_ne!(text, good, "{broken}");
        assert!(
            parse_flowchart("business", &text).is_err(),
            "{broken} passed:\n{text}"
        );
    }
}

#[test]
fn every_information_diagram_of_every_sample_keeps_the_contract() {
    let dir = scratch("diagram_contract_information");
    let mut drawn = 0;
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let text = diagram_with(&root, "information", &args, &dir.join("out"));
            let chart = checked("information", &text, &format!("{root} {scope:?}"));
            for (from, label, to) in &chart.edges {
                let fits = match label.as_str() {
                    "1:1" | "1:N" | "N:1" | "N:M" => to.starts_with("entity__"),
                    "statemodel" => to.starts_with("statemodel__"),
                    _ => label.starts_with("variation ") && to.starts_with("variation__"),
                };
                assert!(
                    fits && from.starts_with("entity__"),
                    "{root} {scope:?}: {from} -{label}-> {to}"
                );
            }
            drawn += usize::from(!chart.edges.is_empty());
        }
    }
    assert!(drawn > 20, "only {drawn} information diagrams draw an edge");
}

#[test]
fn an_information_diagram_draws_the_relates_of_the_er_diagram() {
    let dir = scratch("diagram_contract_information_er");
    let mut relates = 0;
    for root in roots() {
        for scope in scopes(&root) {
            let args: Vec<&str> = scope.iter().map(String::as_str).collect();
            let info = parse_flowchart(
                "information",
                &diagram_with(&root, "information", &args, &dir.join("i")),
            )
            .unwrap();
            let er = er_relates(&diagram_with(&root, "er", &args, &dir.join("e")));
            let drawn: BTreeSet<(String, String, String)> = info
                .edges
                .iter()
                .filter(|(_, label, _)| ["1:1", "1:N", "N:1", "N:M"].contains(&label.as_str()))
                .cloned()
                .collect();
            assert_eq!(drawn, er, "{root} {scope:?}");
            let entities: BTreeSet<&String> = info
                .nodes
                .iter()
                .filter(|(_, kind)| *kind == "entity")
                .map(|(id, _)| id)
                .collect();
            assert!(
                er.iter()
                    .all(|(from, _, to)| entities.contains(from) && entities.contains(to)),
                "{root} {scope:?}"
            );
            relates += drawn.len();
        }
    }
    assert!(relates > 50, "only {relates} relates compared");
}
