//! The diagram encoding contract, checked on every sample for each kind:
//! each `.mmd` opens with the `%%` header, names each node `<kind>__<DSL id>` (the same id in
//! every kind; a variation's value is `variation__<Variation>__<value>`), tags a flowchart
//! node `:::<kind>` in its kind's shape (`rdra_ish_emit::node_shape`), draws edges only
//! between declared nodes, leaves an edge unlabelled exactly when its ends imply the relation
//! (`rdra_ish_emit::implied_relation`), and lists in `kinds:` exactly the kinds its body draws.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The diagram a `%% rdra-ish diagram=` line names.
fn diagram_kind(name: &str) -> rdra_ish_emit::Diagram {
    rdra_ish_emit::Diagram::from_name(name).unwrap_or_else(|| panic!("`{name}` is not a diagram"))
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every samples.txt root.
fn roots() -> Vec<String> {
    std::fs::read_to_string(repo().join("samples.txt"))
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The roots outside the formal-verification fixtures, several of which violate their
/// lifecycle on purpose and so are refused by `state`, the one kind that reads it.
fn lifecycle_roots() -> Vec<String> {
    roots()
        .into_iter()
        .filter(|root| !root.starts_with("samples/formal-verification"))
        .collect()
}

fn buc_ids(root: &str) -> Vec<String> {
    ids(root, "buc")
}

/// The ids `list --kind <kind>` prints for `root`.
fn ids(root: &str, kind: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(repo())
        .args(["list", root, "--kind", kind, "--format", "csv"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|line| line.split(',').next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// `rdra-ish diagram <root> --kind <kind> [--buc <id>]`, the written `.mmd`.
fn diagram(root: &str, kind: &str, buc: Option<&str>, out: &Path) -> String {
    match buc {
        Some(buc) => diagram_with(root, kind, &["--buc", buc], out),
        None => diagram_with(root, kind, &[], out),
    }
}

/// `rdra-ish diagram <root> --kind <kind> <args>`, the written `.mmd`.
fn diagram_with(root: &str, kind: &str, args: &[&str], out: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(repo())
        .args(["diagram", root, "--kind", kind])
        .args(args)
        .arg("--out")
        .arg(out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{root} {kind} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(out.with_extension("mmd")).unwrap()
}

/// The scopes each flowchart kind is checked in: the whole model, then each BUC.
fn scopes(root: &str) -> Vec<Vec<String>> {
    std::iter::once(Vec::new())
        .chain(
            buc_ids(root)
                .into_iter()
                .map(|buc| vec!["--buc".to_string(), buc]),
        )
        .collect()
}

/// Parse and check `text` as a `kind` flowchart, panicking with the diagram on a breach.
fn checked(kind: &str, text: &str, scope: &str) -> Flowchart {
    parse_flowchart(kind, text)
        .and_then(|chart| check_flowchart(&chart).map(|()| chart))
        .unwrap_or_else(|error| panic!("{scope}: {error}\n{text}"))
}

/// What a flowchart under the contract says, read line by line. Any line the contract
/// does not define fails the parse.
#[derive(Default)]
struct Flowchart {
    header_kinds: BTreeSet<String>,
    /// Node id to its `:::` class.
    nodes: HashMap<String, String>,
    /// `(from, relation, to)`: the label, or for an unlabelled edge the relation its ends
    /// imply.
    edges: Vec<(String, String, String)>,
    /// The edges written without a label.
    unlabelled: BTreeSet<(String, String, String)>,
    /// Each edge's arrow: `-->`, `-.->`, or `---`.
    arrows: HashMap<(String, String, String), String>,
    /// The edges a `%% derived` line follows: rdra-ish derived them from other facts.
    derived: BTreeSet<(String, String, String)>,
    class_defs: BTreeSet<String>,
}

/// A node line, `<id><open>"<label>"<close>:::<kind>`, as `(id, kind)`: `None` when the line
/// is no node, an error when it is one in a shape other than its kind's.
fn parse_node(line: &str) -> Option<Result<(String, String), String>> {
    let end = line.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    let (id, rest) = line.split_at(end);
    if id.is_empty() || !rest.starts_with(['[', '(', '{', '>']) {
        return None;
    }
    let Some((body, kind)) = rest.rsplit_once(":::") else {
        return Some(Err(format!("node without `:::<kind>`: {line}")));
    };
    let Some((open, close)) = rdra_ish_emit::node_shape(kind) else {
        return Some(Err(format!("no shape for kind `{kind}`: {line}")));
    };
    let quoted = body
        .strip_prefix(open)
        .and_then(|b| b.strip_suffix(close))
        .filter(|b| b.len() >= 2 && b.starts_with('"') && b.ends_with('"'));
    Some(match quoted {
        Some(_) => Ok((id.to_string(), kind.to_string())),
        None => Err(format!("a {kind} is drawn `{open}\"..\"{close}`: {line}")),
    })
}

/// An edge line as `(from, arrow, label, to)`: `from arrow|label| to`, or `from arrow to`
/// without a label.
fn parse_edge(line: &str) -> Result<(String, String, Option<String>, String), String> {
    let unparsed = || format!("unparsed line: {line}");
    let (from, rest) = line.split_once(' ').ok_or_else(unparsed)?;
    let (arrow, label, to) = match rest.split_once('|') {
        Some((arrow, rest)) => {
            let (label, to) = rest.rsplit_once("| ").ok_or_else(unparsed)?;
            (arrow, Some(label.to_string()), to)
        }
        None => {
            let (arrow, to) = rest.split_once(' ').ok_or_else(unparsed)?;
            (arrow, None, to)
        }
    };
    if !["-->", "-.->", "---"].contains(&arrow) {
        return Err(format!("unknown arrow `{arrow}`: {line}"));
    }
    if to.contains(' ') {
        return Err(unparsed());
    }
    Ok((from.to_string(), arrow.to_string(), label, to.to_string()))
}

fn parse_flowchart(diagram: &str, text: &str) -> Result<Flowchart, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    if !first.starts_with(&format!("%% rdra-ish diagram={diagram} scope=")) {
        return Err(format!("first line is not the header: {first}"));
    }
    let mut chart = Flowchart::default();
    let mut seen_type = false;
    // `(from, arrow, label, to)` until every node is known, and the edge each `%% derived`
    // line follows.
    let mut raw: Vec<(String, String, Option<String>, String)> = Vec::new();
    let mut derived: Vec<usize> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if let Some(kinds) = trimmed.strip_prefix("%% kinds: ") {
            if seen_type {
                return Err(format!("header line after the type line: {line}"));
            }
            chart.header_kinds = kinds
                .split(' ')
                .filter(|kind| *kind != "(none)")
                .map(str::to_string)
                .collect();
        } else if let Some(rest) = trimmed.strip_prefix("%% derived ") {
            let follows = raw
                .last()
                .is_some_and(|(from, _, _, to)| rest.starts_with(&format!("{from} -> {to} ")));
            if !follows {
                return Err(format!("`%% derived` after another edge: {line}"));
            }
            derived.push(raw.len() - 1);
        } else if trimmed.starts_with("%% ") {
        } else if trimmed == "flowchart LR" || trimmed == "flowchart TD" {
            seen_type = true;
        } else if trimmed == "end"
            || ((trimmed.starts_with("subgraph layer_") || trimmed.starts_with("subgraph group_"))
                && trimmed.ends_with("\"]"))
        {
        } else if let Some(rest) = trimmed.strip_prefix("subgraph ") {
            // A subgraph that is an element itself, such as a business holding its BUCs.
            let (id, _) = rest
                .split_once("[\"")
                .filter(|_| rest.ends_with("\"]"))
                .ok_or_else(|| format!("unparsed subgraph: {line}"))?;
            let (kind, _) = id
                .split_once("__")
                .ok_or_else(|| format!("subgraph id without `<kind>__`: {line}"))?;
            chart.nodes.insert(id.to_string(), kind.to_string());
        } else if let Some((kind, _)) = trimmed
            .strip_prefix("classDef ")
            .filter(|rest| rest.ends_with(';'))
            .and_then(|rest| rest.split_once(' '))
        {
            chart.class_defs.insert(kind.to_string());
        } else if let Some(node) = parse_node(trimmed) {
            let (id, class) = node?;
            chart.nodes.insert(id, class);
        } else {
            raw.push(parse_edge(trimmed)?);
        }
    }
    if !seen_type {
        return Err("no `flowchart LR` or `flowchart TD` line".to_string());
    }
    // An unlabelled edge stands for the one relation its ends imply; a labelled one names a
    // relation the diagram draws between its ends and that they do not imply. An end that is
    // not declared is left to `check_flowchart`.
    for (n, (from, arrow, label, to)) in raw.into_iter().enumerate() {
        let kinds = chart.nodes.get(&from).zip(chart.nodes.get(&to));
        let implied =
            kinds.and_then(|(f, t)| rdra_ish_emit::implied_relation(diagram_kind(diagram), f, t));
        let relation = match (label, kinds) {
            (None, None) => String::new(),
            (None, Some((f, t))) => {
                let relation = implied.ok_or_else(|| {
                    format!("{from} {arrow} {to}: no label, and a {f} to a {t} is not one relation")
                })?;
                chart
                    .unlabelled
                    .insert((from.clone(), relation.to_string(), to.clone()));
                relation.to_string()
            }
            (Some(label), None) => label,
            (Some(label), Some((f, t))) => {
                if let Some(implied) = implied {
                    return Err(format!(
                        "{from} -{label}-> {to}: a {f} to a {t} is `{implied}`, drawn without a label"
                    ));
                }
                let listed = rdra_ish_emit::edge_relations(diagram_kind(diagram), f, t)
                    .any(|r| r.label == rdra_ish_emit::EdgeLabel::Data || r.relation == label);
                if !listed {
                    return Err(format!(
                        "{from} -{label}-> {to}: {diagram} draws no such relation from a {f} to a {t}"
                    ));
                }
                label
            }
        };
        let edge = (from, relation, to);
        if derived.contains(&n) {
            chart.derived.insert(edge.clone());
        }
        chart.arrows.insert(edge.clone(), arrow);
        chart.edges.push(edge);
    }
    Ok(chart)
}

/// The contract's structural rules on one parsed flowchart.
fn check_flowchart(chart: &Flowchart) -> Result<(), String> {
    for (id, class) in &chart.nodes {
        let (kind, dsl_id) = id
            .split_once("__")
            .ok_or_else(|| format!("node id without `<kind>__`: {id}"))?;
        // A variation's value is `variation__<Variation>__<value>`, tagged `:::value`.
        let (kind, dsl_id) = match (class.as_str(), dsl_id.split_once("__")) {
            ("value", Some((variation, value))) if kind == "variation" && !variation.is_empty() => {
                ("value", value)
            }
            _ => (kind, dsl_id),
        };
        if kind != class || dsl_id.is_empty() {
            return Err(format!("node {id} is tagged `:::{class}`"));
        }
    }
    for (from, label, to) in &chart.edges {
        for end in [from, to] {
            if !chart.nodes.contains_key(end) {
                return Err(format!(
                    "edge {from} -{label}-> {to}: {end} is not declared"
                ));
            }
        }
    }
    let body_kinds: BTreeSet<String> = chart.nodes.values().cloned().collect();
    if chart.header_kinds != body_kinds {
        return Err(format!(
            "header kinds {:?} but body kinds {body_kinds:?}",
            chart.header_kinds
        ));
    }
    if chart.class_defs != body_kinds {
        return Err(format!(
            "classDef for {:?} but body kinds {body_kinds:?}",
            chart.class_defs
        ));
    }
    let mut edges = chart.edges.clone();
    edges.sort();
    edges.dedup();
    if edges.len() != chart.edges.len() {
        return Err("an edge is drawn twice".to_string());
    }
    Ok(())
}

/// The flowchart kinds of `diagram`, `screen` aside.
const FLOWCHARTS: [&str; 10] = [
    "rdra",
    "context",
    "requirement",
    "business",
    "usecase",
    "robustness",
    "event",
    "flow",
    "information",
    "condition",
];

/// What a state diagram under the contract says: one composite state per state model, its
/// states, its transitions labelled with use-case ids, and its start and end markers.
#[derive(Default)]
struct StateChart {
    header_kinds: BTreeSet<String>,
    /// `statemodel__<Entity>_<column>` to its label `<Entity>.<column>`.
    models: Vec<(String, String)>,
    /// State id to the state model that declares it.
    states: HashMap<String, String>,
    /// (state model, from, to, use case).
    transitions: Vec<(String, String, String, String)>,
    starts: Vec<String>,
    ends: Vec<String>,
    /// An index form's one note per state model: (state model, label).
    index_notes: Vec<(String, String)>,
    /// (state, class) from the `class <state> <class>` lines.
    classes: Vec<(String, String)>,
    class_defs: BTreeSet<String>,
    legend: bool,
}

fn parse_state(text: &str) -> Result<StateChart, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    if !first.starts_with("%% rdra-ish diagram=state scope=") {
        return Err(format!("first line is not the header: {first}"));
    }
    let mut chart = StateChart::default();
    let mut seen_type = false;
    let mut current: Option<String> = None;
    let mut transition: Option<String> = None;
    for line in lines {
        let trimmed = line.trim();
        if let Some(usecase) = trimmed.strip_prefix("%% transition ") {
            transition = Some(usecase.to_string());
        } else if let Some(kinds) = trimmed.strip_prefix("%% kinds: ") {
            chart.header_kinds = kinds
                .split(' ')
                .filter(|kind| *kind != "(none)")
                .map(str::to_string)
                .collect();
        } else if trimmed.starts_with("%% legend: ") {
            chart.legend = true;
        } else if trimmed.starts_with("%% ") {
        } else if let Some(rest) = trimmed.strip_prefix("classDef ") {
            let name = rest.split(' ').next().unwrap_or_default();
            chart.class_defs.insert(name.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("class ") {
            let (state, class) = rest
                .split_once(' ')
                .ok_or_else(|| format!("unparsed class line: {line}"))?;
            chart.classes.push((state.to_string(), class.to_string()));
        } else if trimmed == "stateDiagram-v2" {
            seen_type = true;
        } else if trimmed == "}" {
            current = current
                .take()
                .map(|_| None)
                .ok_or_else(|| format!("`}}` outside a state model: {line}"))?;
        } else if let Some(rest) = trimmed
            .strip_prefix("state \"")
            .and_then(|rest| rest.strip_suffix(" {"))
        {
            let (label, id) = rest
                .split_once("\" as ")
                .ok_or_else(|| format!("unparsed state model: {line}"))?;
            if current.is_some() {
                return Err(format!("nested state model: {line}"));
            }
            chart.models.push((id.to_string(), label.to_string()));
            current = Some(id.to_string());
        } else {
            let model = current
                .clone()
                .ok_or_else(|| format!("line outside a state model: {line}"))?;
            if let Some(id) = trimmed.strip_prefix("[*] --> ") {
                chart.starts.push(id.to_string());
            } else if let Some(id) = trimmed.strip_suffix(" --> [*]") {
                chart.ends.push(id.to_string());
            } else if let Some(rest) = trimmed.strip_prefix("state \"") {
                let (label, id) = rest
                    .split_once("\" as ")
                    .ok_or_else(|| format!("unparsed state: {line}"))?;
                if id.starts_with("index__") {
                    chart.index_notes.push((model, label.to_string()));
                } else if let Some(other) = chart.states.insert(id.to_string(), model.clone()) {
                    if other != model {
                        return Err(format!("state {id} in both {other} and {model}"));
                    }
                }
            } else {
                let (arm, _label) = trimmed
                    .split_once(" : ")
                    .ok_or_else(|| format!("unparsed line: {line}"))?;
                let usecase = transition
                    .take()
                    .ok_or_else(|| format!("no `%% transition` line before: {line}"))?;
                let (from, to) = arm
                    .split_once(" --> ")
                    .ok_or_else(|| format!("unparsed line: {line}"))?;
                chart
                    .transitions
                    .push((model, from.to_string(), to.to_string(), usecase));
            }
        }
    }
    if !seen_type || current.is_some() {
        return Err("no `stateDiagram-v2` line, or an unclosed state model".to_string());
    }
    Ok(chart)
}

fn check_state(chart: &StateChart) -> Result<(), String> {
    let mut models = BTreeSet::new();
    for (id, label) in &chart.models {
        let expected = id
            .strip_prefix("statemodel__")
            .map(|rest| label.replacen('.', "_", 1) == rest);
        if expected != Some(true) || !models.insert(id) {
            return Err(format!(
                "state model {id} \"{label}\" is not one per column"
            ));
        }
    }
    for (state, model) in &chart.states {
        let label = &chart.models.iter().find(|(id, _)| id == model).unwrap().1;
        let entity = label.split('.').next().unwrap_or_default();
        if !state.starts_with(&format!("state__{entity}_")) {
            return Err(format!("state {state} in {model}"));
        }
    }
    for (model, from, to, usecase) in &chart.transitions {
        for end in [from, to] {
            if chart.states.get(end) != Some(model) {
                return Err(format!("{from} --> {to}: {end} is not a state of {model}"));
            }
        }
        if !usecase.starts_with("usecase__") {
            return Err(format!("{from} --> {to} is labelled {usecase}"));
        }
    }
    for state in chart.starts.iter().chain(&chart.ends) {
        if !chart.states.contains_key(state) {
            return Err(format!("start or end {state} is not declared"));
        }
    }
    for state in chart.states.keys() {
        let leaves = chart
            .transitions
            .iter()
            .any(|(_, from, to, _)| from == state && to != state);
        let unreachable = chart
            .classes
            .iter()
            .any(|(id, class)| id == state && class == "stateunreachable");
        if unreachable {
            if leaves || chart.ends.contains(state) || chart.starts.contains(state) {
                return Err(format!("{state}: an unreachable state has a way in or out"));
            }
        } else if leaves == chart.ends.contains(state) {
            return Err(format!(
                "{state}: an end state must be one with no transition out"
            ));
        }
    }
    check_state_index(chart)?;
    check_state_classes(chart)?;
    let mut body_kinds = BTreeSet::new();
    for (kind, present) in [
        ("statemodel", !chart.models.is_empty()),
        (
            "state",
            !chart.states.is_empty() || !chart.index_notes.is_empty(),
        ),
        ("usecase", !chart.transitions.is_empty()),
    ] {
        if present {
            body_kinds.insert(kind.to_string());
        }
    }
    if chart.header_kinds != body_kinds {
        return Err(format!(
            "header kinds {:?} but body kinds {body_kinds:?}",
            chart.header_kinds
        ));
    }
    Ok(())
}

/// An index form draws one `<n> 状態 / <m> 遷移 (--entity <Entity>)` note per state model and
/// nothing else; a full model has no note.
fn check_state_index(chart: &StateChart) -> Result<(), String> {
    if chart.index_notes.is_empty() {
        return Ok(());
    }
    if chart.index_notes.len() != chart.models.len()
        || !chart.states.is_empty()
        || !chart.transitions.is_empty()
        || !chart.classes.is_empty()
    {
        return Err("an index draws one note per state model and no state".to_string());
    }
    for (model, label) in &chart.index_notes {
        let entity = chart
            .models
            .iter()
            .find(|(id, _)| id == model)
            .and_then(|(_, label)| label.split('.').next())
            .unwrap_or_default();
        let counts = label
            .strip_suffix(&format!(" (--entity {entity})"))
            .and_then(|rest| rest.split_once(" 状態 / "))
            .and_then(|(n, rest)| Some((n, rest.strip_suffix(" 遷移")?)))
            .filter(|(n, m)| n.parse::<usize>().is_ok() && m.parse::<usize>().is_ok());
        if counts.is_none() {
            return Err(format!("index note of {model} is `{label}`"));
        }
    }
    Ok(())
}

/// A class line colours a declared state by what the lifecycle check found; each class is
/// defined, and the legend comment is there.
fn check_state_classes(chart: &StateChart) -> Result<(), String> {
    for (state, class) in &chart.classes {
        if !chart.states.contains_key(state) {
            return Err(format!("class of {state}, which is not declared"));
        }
        if !["stateunreachable", "statedeadlock", "stateend"].contains(&class.as_str()) {
            return Err(format!("{state} has the class {class}"));
        }
        if !chart.class_defs.contains(class) {
            return Err(format!("class {class} has no classDef"));
        }
        let leaves = chart
            .transitions
            .iter()
            .any(|(_, from, to, _)| from == state && to != state);
        let ok = match class.as_str() {
            "stateend" => chart.ends.contains(state),
            "statedeadlock" => leaves,
            _ => true,
        };
        if !ok {
            return Err(format!("{state} is wrongly {class}"));
        }
    }
    let ends: BTreeSet<&String> = chart.ends.iter().collect();
    let end_classed: BTreeSet<&String> = chart
        .classes
        .iter()
        .filter(|(_, class)| class == "stateend")
        .map(|(state, _)| state)
        .collect();
    if ends != end_classed {
        return Err("an end state has no stateend class, or the reverse".to_string());
    }
    let full = !chart.models.is_empty() && chart.index_notes.is_empty();
    if full && !chart.legend {
        return Err("a full state model has no `%% legend:` line".to_string());
    }
    Ok(())
}

/// The entities whose state models the unscoped diagram indexes, from its model labels.
fn indexed_entities(chart: &StateChart) -> Vec<String> {
    let entities: BTreeSet<String> = chart
        .models
        .iter()
        .filter_map(|(_, label)| label.split('.').next().map(str::to_string))
        .collect();
    entities.into_iter().collect()
}

/// The columns a state header line `%% <prefix>: <a> <b> (<why>)` names.
fn header_columns(text: &str, prefix: &str) -> BTreeSet<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("%% {prefix}: ")))
        .and_then(|rest| rest.split_once(" ("))
        .map(|(columns, _)| columns.split(' ').map(str::to_string).collect())
        .unwrap_or_default()
}

/// The participant ids and the message endpoints of a `sequence` diagram, after checking its
/// header and that every message joins two declared participants.
fn parse_sequence(text: &str, scope: &str) -> Result<Vec<String>, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty")?;
    let want = format!("%% rdra-ish diagram=sequence scope={scope} model=");
    if !header.starts_with(&want) {
        return Err(format!("header: {header}"));
    }
    let kinds = lines
        .next()
        .and_then(|l| l.strip_prefix("%% kinds: "))
        .ok_or("no kinds line")?;
    if !lines.next().is_some_and(|l| l.starts_with("%% omitted: ")) {
        return Err("no omitted line".to_string());
    }
    if lines.next() != Some("sequenceDiagram") {
        return Err("no sequenceDiagram".to_string());
    }
    let mut participants: Vec<String> = Vec::new();
    let mut drawn_kinds: BTreeSet<String> = BTreeSet::new();
    let mut messages = 0;
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("participant ")
            .or_else(|| trimmed.strip_prefix("actor "))
        {
            let (id, _) = rest.split_once(" as ").ok_or(format!("alias: {line}"))?;
            let (kind, _) = id.split_once("__").ok_or(format!("id: {line}"))?;
            drawn_kinds.insert(kind.to_string());
            participants.push(id.to_string());
        } else if trimmed.starts_with("alt ") || trimmed.starts_with("else ") || trimmed == "end" {
            // A guard's `alt` / `else guard fails`, or one arm per target of a choice.
        } else if let Some(rest) = trimmed.strip_prefix("Note over ") {
            let (id, _) = rest.split_once(": ").ok_or(format!("note: {line}"))?;
            if !participants.iter().any(|p| p == id) {
                return Err(format!("note over an undeclared {id}"));
            }
        } else {
            let (ends, _) = trimmed.split_once(": ").ok_or(format!("message: {line}"))?;
            let (from, to) = ends
                .split_once(" ->> ")
                .or_else(|| ends.split_once(" -->> "))
                .or_else(|| ends.split_once(" -) "))
                .ok_or(format!("arrow: {line}"))?;
            for id in [from, to] {
                if !participants.iter().any(|p| p == id) {
                    return Err(format!("{id} is not a participant: {line}"));
                }
            }
            messages += 1;
        }
    }
    let listed: BTreeSet<String> = kinds.split(' ').map(str::to_string).collect();
    if listed != drawn_kinds {
        return Err(format!("kinds {listed:?} but drawn {drawn_kinds:?}"));
    }
    let unique: BTreeSet<&String> = participants.iter().collect();
    if unique.len() != participants.len() || messages == 0 && participants.len() > 1 {
        return Err("duplicate participant or no message".to_string());
    }
    Ok(participants)
}

/// `(child, cardinality, parent)` for each relationship line of an `erDiagram`.
fn er_relates(text: &str) -> BTreeSet<(String, String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim().split(' ');
            let (from, glyph, to) = (parts.next()?, parts.next()?, parts.next()?);
            let card = match glyph {
                "||--||" => "1:1",
                "||--o{" | "||--|{" => "1:N",
                "}o--||" | "}|--||" => "N:1",
                "}o--o{" => "N:M",
                _ => return None,
            };
            Some((
                format!("entity__{from}"),
                card.to_string(),
                format!("entity__{to}"),
            ))
        })
        .collect()
}

/// A flow model with lanes, which no sample declares: `performs(Actor, Step)`.
const LANES: &str = r#"
actor Clerk "Clerk"
actor Courier "Courier"
actor Manager "Manager"
buc BucShip "Ship"
flow ShipFlow "Ship flow"
flow ReturnFlow "Return flow"
step Pick "Pick"
step Pack "Pack"
step Handover "Handover"
step Receive "Receive"
usecase PickItems "Pick items"
contains(BucShip, ShipFlow)
contains(BucShip, ReturnFlow)
contains(ShipFlow, Pick)
contains(ShipFlow, Pack)
contains(ShipFlow, Handover)
contains(ReturnFlow, Receive)
precedes(Pick, Pack)
precedes(Pack, Handover)
covers(Pick, PickItems)
performs(Clerk, Pick)
performs(Clerk, Pack)
performs(Clerk, Receive)
performs(Courier, Handover)
performs(Manager, Handover)
"#;

/// The flow contract on one parsed flow diagram: ordering edges between steps, `covers`
/// from a step to an action, condition, variation, or timer, `performs` from an actor to a step, and
/// each lane `lane__<Flow>__<Actor>` inside a drawn flow.
fn check_flow(chart: &Flowchart) -> Result<(), String> {
    for (from, label, to) in &chart.edges {
        let fits = match label.as_str() {
            "precedes" | "branches" | "excepts" | "repeats" => {
                from.starts_with("step__") && to.starts_with("step__")
            }
            "covers" => {
                from.starts_with("step__")
                    && [
                        "usecase__",
                        "api__",
                        "event__",
                        "timer__",
                        "condition__",
                        "variation__",
                    ]
                    .iter()
                    .any(|kind| to.starts_with(kind))
            }
            "performs" => from.starts_with("actor__") && to.starts_with("step__"),
            _ => false,
        };
        if !fits {
            return Err(format!("{from} -{label}-> {to}"));
        }
    }
    for (id, kind) in &chart.nodes {
        if ![
            "flow",
            "step",
            "lane",
            "actor",
            "usecase",
            "api",
            "event",
            "timer",
            "condition",
            "variation",
        ]
        .contains(&kind.as_str())
        {
            return Err(format!("a {kind} node {id}"));
        }
        if kind == "lane" {
            let flow = id
                .strip_prefix("lane__")
                .and_then(|rest| rest.split_once("__"))
                .map(|(flow, _)| format!("flow__{flow}"));
            if !flow.is_some_and(|flow| chart.nodes.contains_key(&flow)) {
                return Err(format!("lane {id} outside a drawn flow"));
            }
        }
    }
    Ok(())
}

/// The ids the `%% derived <from> -> <to> via ...` line of `text` names.
fn derived_vias(text: &str, from: &str, to: &str) -> Vec<String> {
    let prefix = format!("%% derived {from} -> {to} via ");
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(|vias| vias.split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

/// The requirement ids that sit in the diagram's `subgraph group_out_of_scope`.
fn apart_requirements(text: &str) -> BTreeSet<String> {
    let mut apart = BTreeSet::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("subgraph group_out_of_scope[") {
            inside = true;
        } else if line == "end" {
            inside = false;
        } else if inside {
            if let Some(Ok((id, _))) = parse_node(line) {
                apart.insert(id);
            }
        }
    }
    apart
}

/// The arrow `chart` draws for `edge`.
fn arrow_of<'a>(chart: &'a Flowchart, edge: &(String, String, String)) -> &'a str {
    chart
        .arrows
        .get(edge)
        .unwrap_or_else(|| panic!("no arrow for {edge:?}"))
}

/// The ids `lint <root>` reports under `rule`.
fn lint_subjects(root: &str, rule: &str) -> BTreeSet<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
        .current_dir(repo())
        .args(["lint", root, "--format", "json"])
        .output()
        .unwrap();
    let issues: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{root}: {error}: {output:?}"));
    issues
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["rule"] == rule)
        .inspect(|issue| assert_eq!(issue["severity"], "info", "{issue}"))
        .map(|issue| issue["subject_id"].as_str().unwrap().to_string())
        .collect()
}

#[path = "diagram_contract/views_1.rs"]
mod views_1;
#[path = "diagram_contract/views_2.rs"]
mod views_2;
#[path = "diagram_contract/views_3.rs"]
mod views_3;
